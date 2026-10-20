-- tpf3mp/geom.lua -- edge geometry: Hermite curves and "which edge is this
-- point on".
--
-- Ported from TpF2 Multiplayer's mp/geom.lua (MIT, tpf2-multiplayer by
-- silver2127, github.com/silver2127/tpf2-multiplayer, 0.6.1.12), which had
-- ported the mid-span split from the older mp_bridge mod. What changed: the
-- functions take the edges to search as a plain list instead of walking the
-- engine's node-to-edge maps themselves, so they run anywhere, tests
-- included; tpf3mp/engine.lua reads the maps. Tolerances are TPF2's
-- measured ones (docs/BUILDING.md) until TPF3 is measured.
--
-- An edge here is { a = {x,y,z}, b = {x,y,z}, ta = {x,y,z}, tb = {x,y,z} }
-- in metres (node0's and node1's positions and the Hermite tangents), plus
-- whatever the caller keeps on it.

local geom = {}

-- How far off a road's centreline a point may be and still be on it: a
-- road's half-width plus margin (a rail touching a road measured 4.5 m off).
geom.SPLIT_EPS = 5.0
-- The same for a track, which has no width to speak of: standard parallel
-- spacing is about 5 m, so 5 m welded new track to its neighbour.
geom.SPLIT_EPS_TRACK = 2.0
-- Nearer an end node than this, a point is that node, not a split.
geom.SPLIT_MIN_DIST = 0.3

function geom.hermitePos(p0, t0, p1, t1, u)
	local u2, u3 = u * u, u * u * u
	local h00, h10 = 2*u3 - 3*u2 + 1, u3 - 2*u2 + u
	local h01, h11 = -2*u3 + 3*u2, u3 - u2
	local r = {}
	for i = 1, 3 do r[i] = h00*p0[i] + h10*t0[i] + h01*p1[i] + h11*t1[i] end
	return r
end

function geom.hermiteTangent(p0, t0, p1, t1, u)
	local u2 = u * u
	local g00, g10 = 6*u2 - 6*u, 3*u2 - 4*u + 1
	local g01, g11 = -6*u2 + 6*u, 3*u2 - 2*u
	local r = {}
	for i = 1, 3 do r[i] = g00*p0[i] + g10*t0[i] + g01*p1[i] + g11*t1[i] end
	return r
end

-- The edge of `edges` whose curve passes within `tol` metres of (x, y),
-- horizontally, and the parameter u where it does; nil when none does or
-- the nearest point is within SPLIT_MIN_DIST of an end (that is the end
-- node, not a split). `skip(edge)` excludes edges, for instance those
-- already ending at a node being welded into.
--
-- Sampling is by distance, not a fixed count: a 77 m town road sampled
-- every 7.7 m misses a point that is on it. The best sample is refined by
-- a ternary search on distance.
function geom.edgeContaining(edges, x, y, tol, skip)
	local best, bestD, bestU, bestSteps
	for _, e in ipairs(edges) do
		if not (skip and skip(e)) then
			local a, b, ta, tb = e.a, e.b, e.ta, e.tb
			local span = (b[1]-a[1])^2 + (b[2]-a[2])^2
			local d0 = (a[1]-x)^2 + (a[2]-y)^2
			local d1 = (b[1]-x)^2 + (b[2]-y)^2
			-- a cheap pre-filter: nowhere near either end
			if d0 < span * 4 + 400 or d1 < span * 4 + 400 then
				local len = math.max(math.sqrt(span),
					math.sqrt(ta[1]^2 + ta[2]^2), math.sqrt(tb[1]^2 + tb[2]^2))
				local steps = math.min(400, math.max(19, math.ceil(len / 1.0)))
				for i = 1, steps - 1 do
					local u = i / steps
					local q = geom.hermitePos(a, ta, b, tb, u)
					local d = (q[1]-x)^2 + (q[2]-y)^2
					if d < tol * tol and (not bestD or d < bestD) then
						best, bestD, bestU, bestSteps = e, d, u, steps
					end
				end
			end
		end
	end
	if not best then return nil end
	local a, b, ta, tb = best.a, best.b, best.ta, best.tb
	local lo, hi = math.max(0, bestU - 1 / bestSteps), math.min(1, bestU + 1 / bestSteps)
	for _ = 1, 12 do
		local u1, u2 = lo + (hi - lo) / 3, hi - (hi - lo) / 3
		local q1, q2 = geom.hermitePos(a, ta, b, tb, u1), geom.hermitePos(a, ta, b, tb, u2)
		if (q1[1]-x)^2 + (q1[2]-y)^2 < (q2[1]-x)^2 + (q2[2]-y)^2 then hi = u2 else lo = u1 end
	end
	local u = (lo + hi) / 2
	local q = geom.hermitePos(a, ta, b, tb, u)
	local dA = math.sqrt((q[1]-a[1])^2 + (q[2]-a[2])^2)
	local dB = math.sqrt((q[1]-b[1])^2 + (q[2]-b[2])^2)
	if dA < geom.SPLIT_MIN_DIST or dB < geom.SPLIT_MIN_DIST then return nil end
	return best, u
end

-- The parameter u of the point of the curve (a, ta) to (b, tb) nearest (x, y)
-- horizontally, and its horizontal distance. For the room's replays, which
-- every game must compute alike, whatever its platform: nothing but + - * /
-- and sqrt, which IEEE 754 fixes exactly (no ^, which is the C library's pow),
-- and a fixed number of steps: 64 samples, then 48 steps of a ternary search
-- about the best, to within 1e-9.
function geom.parameterAt(a, ta, b, tb, x, y)
	local function d2(u)
		local q = geom.hermitePos(a, ta, b, tb, u)
		local dx, dy = q[1] - x, q[2] - y
		return dx * dx + dy * dy
	end
	local STEPS = 64
	local best, bestD = 0, d2(0)
	for i = 1, STEPS do
		local u = i / STEPS
		local d = d2(u)
		if d < bestD then best, bestD = u, d end
	end
	local lo, hi = best - 1 / STEPS, best + 1 / STEPS
	if lo < 0 then lo = 0 end
	if hi > 1 then hi = 1 end
	for _ = 1, 48 do
		local u1, u2 = lo + (hi - lo) / 3, hi - (hi - lo) / 3
		if d2(u1) < d2(u2) then hi = u2 else lo = u1 end
	end
	local u = (lo + hi) / 2
	return u, math.sqrt(d2(u))
end

-- How near, in three dimensions, an end of the edge that takes a signal is
-- the same end of the edge it stood on, and how alike their directions
-- there are (the dot of their unit tangents).
geom.CONTINUE_TOL = 0.5
geom.CONTINUE_DOT = 0.995

-- Whether the edge `to` continues the edge `from` (both { a, b, ta, tb })
-- from one of its ends, the same way: it starts where `from` starts, or
-- ends where it ends, in the same direction there. So the track tool keeps
-- a signal on a track it cuts (seen on build 40420, 2026-10-17): on the
-- piece that goes on from its old track's end, the one before the cut or
-- the one after it, even where the tool moved a node near the cut and the
-- signal now stands past it; the game places it on that piece. Returns
-- true, or nil and why not. Every game computes it alike (sqrt only).
local function aligned(t1, t2)
	local n1 = math.sqrt(t1[1] * t1[1] + t1[2] * t1[2] + t1[3] * t1[3])
	local n2 = math.sqrt(t2[1] * t2[1] + t2[2] * t2[2] + t2[3] * t2[3])
	if n1 == 0 or n2 == 0 then return false end
	return (t1[1] * t2[1] + t1[2] * t2[2] + t1[3] * t2[3]) / (n1 * n2) >= geom.CONTINUE_DOT
end

function geom.continues(from, to)
	local function near(p, q)
		local dx, dy, dz = p[1] - q[1], p[2] - q[2], p[3] - q[3]
		return dx * dx + dy * dy + dz * dz <= geom.CONTINUE_TOL * geom.CONTINUE_TOL
	end
	if near(from.a, to.a) and aligned(from.ta, to.ta) then return true end
	if near(from.b, to.b) and aligned(from.tb, to.tb) then return true end
	return nil, "a signal onto an edge that does not go on from its track"
end

-- How near the edge that takes a signal must run where the signal stood,
-- horizontally and in height.
geom.STAND_TOL = 0.5

-- Whether the edge `to` runs where a signal stood at `at` along the edge
-- `from` (both { a, b, ta, tb }), the same way: its centreline passes
-- within STAND_TOL of that place, horizontally and in height, and its
-- direction there is the old edge's (CONTINUE_DOT). So the track tool
-- puts a signal back near a node it removes (seen on build 40420,
-- 2026-10-17): on the piece that now runs where it stood, which may be the
-- one that took over the track beyond that node. Returns true, or nil and
-- why not. Every game computes it alike (sqrt only, fixed steps).
function geom.runsThrough(from, at, to)
	local spot = geom.hermitePos(from.a, from.ta, from.b, from.tb, at)
	local u, d = geom.parameterAt(to.a, to.ta, to.b, to.tb, spot[1], spot[2])
	local h = geom.hermitePos(to.a, to.ta, to.b, to.tb, u)[3]
	if d > geom.STAND_TOL or math.abs(h - spot[3]) > geom.STAND_TOL then
		return nil, "a signal onto an edge that does not run where it stood"
	end
	if not aligned(geom.hermiteTangent(from.a, from.ta, from.b, from.tb, at),
		geom.hermiteTangent(to.a, to.ta, to.b, to.tb, u)) then
		return nil, "a signal onto an edge that runs the other way where it stood"
	end
	return true
end

-- Whether a signal that stood at `at` along `from` may go onto `to`: a
-- piece that goes on from one of its old track's ends (continues), or one
-- that runs where it stood, the same way (runsThrough). The capture and
-- every game's replay ask the same.
function geom.keepsSignal(from, at, to)
	if geom.continues(from, to) then return true end
	local ok = geom.runsThrough(from, at, to)
	if ok then return true end
	return nil, "a signal onto an edge that neither goes on from its track nor runs where it stood"
end

-- Nearest point of the curve to the cursor's forward viewing ray. The
-- terrain hit lies beyond elevated track, so projecting that hit vertically
-- onto the track can move a signal tens of metres along a bridge.
function geom.parameterOnRay(a, ta, b, tb, eye, hit)
	local dx, dy, dz = hit[1]-eye[1], hit[2]-eye[2], hit[3]-eye[3]
	local length2 = dx*dx + dy*dy + dz*dz
	if length2 <= 0 then return nil end
	local function distance2(u)
		local q = geom.hermitePos(a, ta, b, tb, u)
		local x, y, z = q[1]-eye[1], q[2]-eye[2], q[3]-eye[3]
		local t = math.max(0, (x*dx + y*dy + z*dz) / length2)
		x, y, z = x-t*dx, y-t*dy, z-t*dz
		return x*x + y*y + z*z
	end
	local best, bestD = 0, distance2(0)
	for i = 1, 64 do
		local u, d = i / 64, distance2(i / 64)
		if d < bestD then best, bestD = u, d end
	end
	local lo, hi = math.max(0, best - 1/64), math.min(1, best + 1/64)
	for _ = 1, 48 do
		local u1, u2 = lo + (hi-lo)/3, hi - (hi-lo)/3
		if distance2(u1) < distance2(u2) then hi = u2 else lo = u1 end
	end
	return (lo + hi) / 2
end

return geom
