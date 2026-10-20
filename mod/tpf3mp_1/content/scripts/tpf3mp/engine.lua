-- tpf3mp/engine.lua -- Transport Fever 3's street and track tools and its
-- network, behind the plain interfaces tpf3mp/roads.lua takes.
--
-- What it reads, as build 40408 has it (the game's api/tealdef and the build
-- probe, tools/probe/tf3/tpf3mp_buildprobe_1): the street and track tools
-- hand game scripts a proposal (builder.proposalCreate's first parameter)
-- whose .proposal is a StreetProposal:
--
-- - addedNodes: the new nodes, entity < 0, comp.position;
-- - addedSegments: the new edges, entity < 0, type 0 street / 1 track, comp a
--   BaseEdge (node0, node1, tangent0, tangent1, type NORMAL / BRIDGE /
--   TUNNEL, typeIndex, roadTemplate and roadStyle, resource names, and the
--   stops and signals on it, objects);
-- - removedSegments and removedNodes: the existing edges and nodes it
--   removes.
--
-- A street drawn onto another's middle (seen): the old street's node nearest
-- the new junction is removed with its two edges, and the old street is
-- rebuilt from its neighbours through the junction, in its own template.
--
-- The lists are the game's vectors: read by index, never with pairs().

local function module(name)
	local loaded = package and package.loaded and package.loaded["tpf3mp." .. name]
	if loaded then return loaded end
	if ug_require then return ug_require("tpf3mp_1::/scripts/tpf3mp/" .. name .. ".lua") end
	return require("tpf3mp." .. name)
end

local roads = module("roads")
local geom = module("geom")
local junctions = module("junctions")

local engine = {}

local function get(value, key)
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

-- The game's vector (or a table) as a Lua array.
local function list(v)
	if v == nil then return {} end
	local ok, n = pcall(function() return #v end)
	if not ok or type(n) ~= "number" then error("a list it cannot read", 0) end
	local out = {}
	for i = 1, n do out[i] = v[i] end
	return out
end

local function vec3(v)
	if v == nil then return nil end
	local x, y, z = get(v, "x"), get(v, "y"), get(v, "z")
	if x == nil then x, y, z = get(v, 1), get(v, 2), get(v, 3) end
	return { x, y, z }
end

-- The game's enums, under api.type.enum ("enum" is a word in Teal, which
-- writes api.type["enum"]).
local function enum(name)
	local enums = api.type.enum
	local e = enums and enums[name]
	if e == nil then error("no api.type.enum." .. name, 0) end
	return e
end

local function nodePos(id)
	local p
	pcall(function()
		local c = api.engine.getComponent(id, api.type.ComponentType.BASE_NODE)
		if c and c.position then p = vec3(c.position) end
	end)
	return p
end

-- A node's edges in one network, from the street system.
local function nodeEdges(id, network)
	local edges
	pcall(function()
		local streets = api.engine.system.streetSystem
		if network == "Track" then edges = streets.getNodeTrackSegments(id) else edges = streets.getNodeStreetSegments(id) end
	end)
	return edges
end

-- The world as tpf3mp/roads.lua asks it. Only the nodes a proposal names
-- are read: no edge of the map is walked.
function engine.world()
	local world = {}
	world.nodePos = nodePos
	function world.nodeNetwork(id)
		for _, network in ipairs({ "Street", "Track" }) do
			local edges = nodeEdges(id, network)
			if edges ~= nil and #list(edges) > 0 then return network end
		end
		return nil
	end
	return world
end

local function networkOf(seg)
	local kind = get(seg, "type")
	if kind == 0 then return "Street" end
	if kind == 1 then return "Track" end
	error("an edge of type " .. tostring(kind), 0)
end

local function resName(v, what)
	if type(v) ~= "string" or v == "" then error(what .. " is not a resource name: " .. tostring(v), 0) end
	return v
end

-- A bridge's or tunnel's type, by its name.
local function typeName(rep, index)
	local name
	pcall(function() name = api.res[rep].getName(index) end)
	if type(name) ~= "string" or name == "" then error("no " .. rep .. " type " .. tostring(index), 0) end
	return name
end

-- A road style: "" is none.
local function styleName(v)
	if v == "" or v == nil then return nil end
	return resName(v, "roadStyle")
end

-- The stops and signals an edge's component lists, by entity, sorted: an
-- edge replaced without its own leaves them pointing nowhere (on TPF2 that
-- crashed every game at the same step; docs/BUILDING.md), so a build may
-- only rebuild an edge in place with the same ones (engine.fromProposal).
local function objectEntities(c)
	local out = {}
	for _, o in ipairs(list(get(c, "objects"))) do
		local entity = get(o, 1)
		if type(entity) ~= "number" then error("an edge object it cannot read", 0) end
		out[#out + 1] = entity
	end
	table.sort(out)
	return out
end

-- The same as { entity, type } pairs (EdgeObjectType), as the edge lists
-- them.
local function objectPairs(c)
	local out = {}
	for _, o in ipairs(list(get(c, "objects"))) do
		local entity, kind = get(o, 1), get(o, 2)
		if type(entity) ~= "number" or type(kind) ~= "number" then error("an edge object it cannot read", 0) end
		out[#out + 1] = { entity, kind }
	end
	return out
end

-- An edge's decorations (noise barriers, alleys) by name, with the game's
-- flag for each (BaseEdge.edgeDecorations: { decoration id, flag }).
local function decorationsOf(c)
	local out = {}
	for _, d in ipairs(list(get(c, "edgeDecorations"))) do
		local id, flag = get(d, 1), get(d, 2)
		local name
		pcall(function() name = api.res.edgeDecorationRep.getName(id) end)
		out[#out + 1] = { name = resName(name, "an edge decoration"), flag = flag == true }
	end
	return out
end

-- An edge's lanes (BaseEdge.laneConfigs): speed, width, height and offset
-- in the game's units, the direction, and the transport modes as a bit for
-- each TransportMode value. A tram track or a bus lane is here on TF3, not
-- in the template.
local function lanesOf(c)
	local out = {}
	for _, lc in ipairs(list(get(c, "laneConfigs"))) do
		local tm = get(lc, "transportModes")
		local modes = 0
		for m = 0, 15 do
			local on = false
			pcall(function() on = tm[m] == true end)
			if on then modes = modes + 2 ^ m end
		end
		local lane = { forward = get(lc, "forward") == true, modes = modes }
		for _, f in ipairs({ "speed", "width", "height", "offset" }) do
			local v = get(lc, f)
			if type(v) ~= "number" then error("a lane with no " .. f, 0) end
			lane[f] = v
		end
		out[#out + 1] = lane
	end
	return out
end

-- Street precedence is part of the player's proposal, including an explicit zero.
local function precedenceOf(seg)
    local se = get(seg, "streetEdge")
    if se == nil then return nil end
    local a, b = get(se, "precedenceNode0"), get(se, "precedenceNode1")
    if a == nil and b == nil then return nil end
    if type(a) ~= "number" or type(b) ~= "number" or a % 1 ~= 0 or b % 1 ~= 0 then
        error("a street's precedence cannot be read", 0)
    end
    return { node0 = a, node1 = b }
end

local function segment(seg)
	local c = get(seg, "comp")
	if c == nil then error("an edge with no component", 0) end
	local owner = get(seg, "playerOwned")
	local player = owner and get(owner, "player")
	local e = {
		node0 = c.node0, node1 = c.node1,
		network = networkOf(seg),
		tangent0 = vec3(c.tangent0), tangent1 = vec3(c.tangent1),
		-- Where the edge itself ends, which need not be its node: the track
		-- tool starts a track laid onto a street's end on its tram lane
		-- (roads.lua, BUILDING.md "Edge ends").
		position0 = vec3(get(c, "position0")),
		position1 = vec3(get(c, "position1")),
		structure = "Ground",
		template = resName(c.roadTemplate, "roadTemplate"),
		style = styleName(c.roadStyle),
		objects = objectEntities(c),
		typed = objectPairs(c),
		decorations = decorationsOf(c),
		locked = get(c, "roadDevelopmentLocked") == true,
		owned = type(player) == "number" and player >= 0,
		lanes = lanesOf(c),
	}
	if e.network == "Street" then e.precedence = precedenceOf(seg) end
	local types = enum("BaseEdgeType")
	if c.type == types.BRIDGE then
		e.structure = { Bridge = typeName("bridgeTypeRep", c.typeIndex) }
	elseif c.type == types.TUNNEL then
		e.structure = { Tunnel = typeName("tunnelTypeRep", c.typeIndex) }
	elseif c.type ~= types.NORMAL then
		error("an edge of structure " .. tostring(c.type), 0)
	end
	return e
end

-- A removed edge's geometry, as geom.lua takes it: the tangents the
-- proposal lists for it, else its own.
local function removedGeometry(r, posOf)
	local ta, tb = vec3(get(r.comp, "tangent0")), vec3(get(r.comp, "tangent1"))
	if (ta == nil or tb == nil) and type(r.entity) == "number" then
		local c
		pcall(function() c = api.engine.getComponent(r.entity, api.type.ComponentType.BASE_EDGE) end)
		ta, tb = c and vec3(get(c, "tangent0")), c and vec3(get(c, "tangent1"))
	end
	local a, b = posOf(r.node0), posOf(r.node1)
	if not (a and b and ta and tb and type(ta[1]) == "number" and type(tb[1]) == "number") then
		error("a signal's track has no geometry here", 0)
	end
	return { a = a, b = b, ta = ta, tb = tb }
end

-- Where along the removed edge `r` its signal `entity` stands, and its
-- construction; or raises. A waypoint is an edge object of the same type
-- (SIGNAL), and lines name it: it never moves onto another edge. The game
-- tells it by its signal list (SIGNAL_LIST: each signal's type, a
-- Signal.Type, WAYPOINT 2 on build 40420), whatever its construction, a
-- mod's included.
local function signalOn(entity)
	local C = api.type.ComponentType
	local c, signals, waypoint
	pcall(function()
		c = api.engine.getComponent(entity, C.EDGE_OBJECT)
		signals = get(api.engine.getComponent(entity, C.SIGNAL_LIST), "signals")
		waypoint = api.type.Signal.Type.WAYPOINT
	end)
	local p, model = c and get(c, "param"), c and get(c, "edgeObjectConstruction")
	if type(p) ~= "number" or type(model) ~= "string" or model == "" then
		error("a signal it cannot read", 0)
	end
	local each = signals ~= nil and list(signals) or {}
	if #each == 0 or type(waypoint) ~= "number" then error("a signal it cannot read", 0) end
	for _, s in ipairs(each) do
		local kind = get(s, "type")
		if type(kind) ~= "number" then error("a signal it cannot read", 0) end
		if kind == waypoint then error("a build that moves a waypoint", 0) end
	end
	return p, model
end

-- A signal the track tool puts back where it stood. A track started close
-- to a node of a track with signals: the tool removes that node, redraws
-- the track through its own, and on an edge whose node it removes it
-- replaces the signal with a new one on the piece that goes on from the
-- old edge's other end (seen on build 40420, 2026-10-17, run-1017-033551:
-- the signal in edgeObjectsToRemove, one record in edgeObjectsToAdd with
-- category 2 and nothing else that reads, no model, place or settings,
-- and that piece listing it as a new entity). The piece may also run
-- across the removed node, from the tool's own node or the old track's end
-- before it to one past it (run-1017-pr165-p2, run-1017-refit). Each new
-- signal is taken as the old one, kept on that piece under its own
-- entity (keptInPlace, as a split's): every game's build keeps its
-- construction and its settings,
-- which the tool's record does not say. Only one for one: each record a
-- signal on one new track edge, each removal a signal (no waypoint) on one
-- removed track edge, each new signal on a piece that only one removed
-- signal goes onto: a stretch of its old track (both ends on that
-- signal's edge or on a removed track edge that meets it at a node the
-- build removes), not rebuilt between the signal's edge's own nodes, and
-- that runs where the signal stood, the same way (geom.runsThrough);
-- anything else refused as before. Behind
-- acceptance.lua's refitSignals, which only the capture asks: the room
-- carries it as a split's Link.kept. Returns the old signals so kept, by
-- entity.
local SIGNAL_CATEGORY = 2
local function refitSignals(capture, connection, posOf)
	local adds, removes = capture.objectAdds or {}, capture.objectRemoves or {}
	if #adds == 0 and #removes == 0 then return {} end
	local refused = "a build with a stop or signal"
	if connection or #adds ~= #removes then error(refused, 0) end
	local SIGNAL = enum("EdgeObjectType").SIGNAL
	-- The removed signals, by entity, on the removed track edge each stands on.
	local on = {}
	for k, r in ipairs(capture.removed) do
		for _, o in ipairs(r.typed or {}) do
			if on[o[1]] ~= nil then error("an edge object on two edges", 0) end
			on[o[1]] = { k = k, kind = o[2] }
		end
	end
	local removing = {}
	for _, entity in ipairs(removes) do
		local src = type(entity) == "number" and entity >= 0 and on[entity]
		if not src or removing[entity] or src.kind ~= SIGNAL or capture.removed[src.k].network ~= "Track" then
			error(refused, 0)
		end
		-- A waypoint, which lines name, is never put back for another.
		src.at = signalOn(entity)
		removing[entity] = src
	end
	-- The new signals, by the entity the proposal gives each.
	local new = {}
	for _, rec in ipairs(adds) do
		local id = get(rec, "resultEntity")
		if get(rec, "category") ~= SIGNAL_CATEGORY or type(id) ~= "number" or id >= 0 or new[id] ~= nil then
			error(refused, 0)
		end
		new[id] = false
	end
	local placed = {}
	for _, e in ipairs(capture.edges) do
		for i, o in ipairs(e.typed or {}) do
			if removing[o[1]] then error(refused, 0) end
			if o[1] < 0 then
				if new[o[1]] ~= false or o[2] ~= SIGNAL or e.network ~= "Track" then error(refused, 0) end
				new[o[1]] = true
				placed[#placed + 1] = { e = e, i = i }
			end
		end
	end
	for _, seen in pairs(new) do
		if not seen then error(refused, 0) end
	end
	-- The removed track edges by the nodes the build removes that they
	-- end at: across such a node the track goes on as another removed edge.
	local gone = {}
	for _, n in ipairs(capture.removedNodes) do gone[n.id] = {} end
	for k, r in ipairs(capture.removed) do
		if r.network == "Track" then
			for _, n in ipairs({ r.node0, r.node1 }) do
				if gone[n] then gone[n][#gone[n] + 1] = k end
			end
		end
	end
	local geometry = {}
	local function geometryOf(k)
		geometry[k] = geometry[k] or removedGeometry(capture.removed[k], posOf)
		return geometry[k]
	end
	-- Whether a place lies on the signal's old track: its edge, or a removed
	-- track edge that meets it at a node the build removes (within
	-- STAND_TOL, horizontally and in height).
	local function onOldTrack(src, p)
		local r = capture.removed[src.k]
		local edges = { src.k }
		for _, n in ipairs({ r.node0, r.node1 }) do
			for _, k in ipairs(gone[n] or {}) do
				if k ~= src.k then edges[#edges + 1] = k end
			end
		end
		for _, k in ipairs(edges) do
			local g = geometryOf(k)
			local u, d = geom.parameterAt(g.a, g.ta, g.b, g.tb, p[1], p[2])
			local h = geom.hermitePos(g.a, g.ta, g.b, g.tb, u)[3]
			if d <= geom.STAND_TOL and math.abs(h - p[3]) <= geom.STAND_TOL then return true end
		end
		return false
	end
	-- Whether the piece is a stretch of the signal's old track: both its
	-- ends on it. The tool sets its own nodes on the old track about the
	-- node it removes, before it, past it or both (run-1017-refit: one 20 m
	-- before, one 29 m past, the signal put back on the piece between).
	local function tied(src, to)
		return onOldTrack(src, to.a) and onOldTrack(src, to.b)
	end
	-- Each new signal the one removed signal whose track goes on to its
	-- piece, which runs where that signal stood, the same way.
	local function standsOn(src, e, to)
		local r = capture.removed[src.k]
		if (e.node0 == r.node0 and e.node1 == r.node1) or (e.node0 == r.node1 and e.node1 == r.node0) then
			return false
		end
		return tied(src, to) and geom.runsThrough(geometryOf(src.k), src.at, to) == true
	end
	local used, pairsOf = {}, {}
	for _, p in ipairs(placed) do
		local e = p.e
		local to = { a = posOf(e.node0), b = posOf(e.node1), ta = e.tangent0, tb = e.tangent1 }
		if not (to.a and to.b and to.ta and to.tb) then error("a signal's new track has no place here", 0) end
		local match
		for entity, src in pairs(removing) do
			if standsOn(src, e, to) then
				if match ~= nil then error("a build that replaces signals it cannot tell apart", 0) end
				match = entity
			end
		end
		if match == nil or used[match] then error("a build that replaces a signal elsewhere", 0) end
		used[match] = true
		pairsOf[#pairsOf + 1] = { p = p, entity = match }
	end
	if module("acceptance").refitSignals ~= true then
		error("a signal the track tool puts back awaits two-player game acceptance", 0)
	end
	local refitted = {}
	for _, m in ipairs(pairsOf) do
		refitted[m.entity] = true
		local e = m.p.e
		e.typed[m.p.i] = { m.entity, SIGNAL }
		e.objects = {}
		for _, o in ipairs(e.typed) do e.objects[#e.objects + 1] = o[1] end
		table.sort(e.objects)
	end
	return refitted
end

-- Stops and signals move with a build on an edge it rebuilds in place: a
-- new edge between the same places, in the same direction, as a removed
-- one, listing exactly its objects (a modifier tool's, a road drawn
-- through). Each game's build then gives the new edge the objects of the
-- one it replaces (tpf3mp/apply.lua).
-- A signal also moves onto another track the build makes where the track
-- it stood on is cut (a track started or ended between its signals): the
-- piece that goes on from one of its old track's ends, the same way
-- (geom.continues), where the tool put it; a signal the tool puts back
-- (refitSignals) also onto one that runs where it stood, the same way
-- (geom.keepsSignal, as every game's replay asks). The edge that takes it says
-- which, by the removal, the place along it and its construction (`kept`,
-- action::KeptSignal), as the tool decided.
-- Anything else is refused: a new stop or signal, one dropped, a stop or a
-- waypoint carried onto another edge. `connection` true for a
-- construction's streets, which keep no signal on another edge (every
-- game's build of them peels and refuses it, apply.lua keptSignals).
function engine.keptInPlace(capture, connection)
	local newPos = {}
	for _, n in ipairs(capture.nodes) do newPos[n.id] = n.pos end
	for _, n in ipairs(capture.removedNodes) do newPos[n.id] = n.pos end
	local function posOf(id)
		if newPos[id] then return newPos[id] end
		return nodePos(id)
	end
	local function same(a, b)
		return a and b and math.abs(a[1] - b[1]) < 0.05 and math.abs(a[2] - b[2]) < 0.05
			and math.abs(a[3] - b[3]) < 0.05
	end
	local function key(list) return table.concat(list, ",") end
	local refitted = refitSignals(capture, connection, posOf)
	local placed, claimed, moving = {}, {}, {}
	for _, e in ipairs(capture.edges) do
		if #e.objects > 0 then
			for _, o in ipairs(e.objects) do
				if o < 0 then error("a build that adds a stop or signal", 0) end
			end
			local found
			for k, r in ipairs(capture.removed) do
				if not placed[k] and key(r.objects) == key(e.objects)
					and same(posOf(r.node0), posOf(e.node0)) and same(posOf(r.node1), posOf(e.node1)) then
					found = k
					break
				end
			end
			if found == nil then
				moving[#moving + 1] = e
			else
				placed[found] = true
				for _, o in ipairs(e.objects) do claimed[o] = true end
			end
		end
	end
	if #moving > 0 and connection then error("a build that moves a stop or signal", 0) end
	if #moving > 0 then
		-- Each object of a removed edge not rebuilt in place, by entity.
		local from = {}
		for k, r in ipairs(capture.removed) do
			if not placed[k] then
				for _, o in ipairs(r.typed or {}) do
					if from[o[1]] ~= nil then error("an edge object on two edges", 0) end
					from[o[1]] = { k = k, kind = o[2] }
				end
			end
		end
		local SIGNAL
		pcall(function() SIGNAL = enum("EdgeObjectType").SIGNAL end)
		local geometry = {}
		for _, e in ipairs(moving) do
			local to
			e.kept = {}
			for _, o in ipairs(e.typed or {}) do
				local src = from[o[1]]
				if src == nil or claimed[o[1]] or o[2] ~= src.kind then
					error("a build that moves a stop or signal", 0)
				end
				local r = capture.removed[src.k]
				if SIGNAL == nil or o[2] ~= SIGNAL or r.network ~= "Track" or e.network ~= "Track" then
					error("a build that moves a stop or signal", 0)
				end
				local p, model = signalOn(o[1])
				geometry[src.k] = geometry[src.k] or removedGeometry(r, posOf)
				to = to or { a = posOf(e.node0), b = posOf(e.node1), ta = e.tangent0, tb = e.tangent1 }
				if not (to.a and to.b and to.ta and to.tb) then error("a signal's new track has no place here", 0) end
				local ok, why
				if refitted[o[1]] then
					ok, why = geom.keepsSignal(geometry[src.k], p, to)
				else
					ok, why = geom.continues(geometry[src.k], to)
				end
				if not ok then error(why, 0) end
				claimed[o[1]] = true
				e.kept[#e.kept + 1] = { removal = src.k - 1, at = p, model = model }
			end
		end
	end
	for _, r in ipairs(capture.removed) do
		for _, o in ipairs(r.objects) do
			if not claimed[o] then error("a build that removes an edge with a stop or signal on it", 0) end
		end
	end
end

-- A tool's proposal as tpf3mp/roads.lua takes it, or raises. `network` is
-- the tool's; nil for the construction tool's, whose street part is taken
-- alone (`constructions` true lets the proposal carry them) in the network
-- of its first new edge. Returns nil for a proposal of nothing (the tool
-- before its first point, a construction with no street part).
-- `constructions` "town" lets the proposal carry town buildings only: the
-- ones a road modifier clears and puts back along the road, which every
-- game's build clears again as the tool's does (ignoreErrors,
-- tpf3mp/apply.lua).
local function townBuildingsOnly(proposal)
	for _, e in ipairs(list(get(proposal, "toRemove"))) do
		-- A town building is a construction that lists its town buildings
		-- (as capture.construction tells them).
		local c = api.engine.getComponent(e, api.type.ComponentType.CONSTRUCTION)
		local buildings = c and get(c, "townBuildings")
		if buildings == nil or #list(buildings) == 0 then error("a build that removes a construction", 0) end
	end
	for _, c in ipairs(list(get(proposal, "toAdd"))) do
		local file = get(c, "fileName")
		if type(file) ~= "string" or not file:find("/buildings/", 1, true) then
			error("a build with constructions", 0)
		end
	end
end

function engine.fromProposal(proposal, network, constructions)
	local street = get(proposal, "proposal")
	if street == nil then error("a proposal with no street proposal", 0) end
	if constructions == "town" then
		townBuildingsOnly(proposal)
	elseif not constructions then
		for _, name in ipairs({ "toAdd", "toRemove" }) do
			if #list(get(proposal, name)) > 0 then error("a build with constructions", 0) end
		end
	end
	-- Stops and signals added or removed: only a signal the track tool puts
	-- back where it stood (refitSignals).
	local objectAdds, objectRemoves = list(get(street, "edgeObjectsToAdd")), list(get(street, "edgeObjectsToRemove"))
	local added, segments, removed = list(get(street, "addedNodes")), list(get(street, "addedSegments")),
		list(get(street, "removedSegments"))
	local removedNodes = list(get(street, "removedNodes"))
	if #added == 0 and #segments == 0 and #removed == 0 and #removedNodes == 0 then
		if #objectAdds > 0 or #objectRemoves > 0 then error("a build with a stop or signal", 0) end
		return nil
	end

	local capture = { network = network, nodes = {}, edges = {}, removed = {}, removedNodes = {},
		junctions = junctions.capture(street), objectAdds = objectAdds, objectRemoves = objectRemoves }
	for _, n in ipairs(added) do
		capture.nodes[#capture.nodes + 1] = { id = n.entity, pos = vec3(n.comp.position) }
	end
	local first
	for _, seg in ipairs(segments) do
		local e = segment(seg)
		capture.edges[#capture.edges + 1] = e
		if network == nil then network = e.network capture.network = network end
		if not first and e.network == network then first = e end
	end
	for _, seg in ipairs(removed) do
		capture.removed[#capture.removed + 1] = { node0 = seg.comp.node0, node1 = seg.comp.node1,
			network = networkOf(seg), objects = objectEntities(seg.comp), typed = objectPairs(seg.comp),
			entity = get(seg, "entity"), comp = seg.comp }
	end
	for _, n in ipairs(removedNodes) do
		capture.removedNodes[#capture.removedNodes + 1] = { id = n.entity, pos = vec3(get(n.comp, "position")) }
	end
	engine.keptInPlace(capture, constructions == true)

	-- The build's own kind: its first edge of the tool's network. The
	-- template names the edge whole on TF3: its lanes, bus lanes and tram
	-- tracks. The schema's bus lane and tram are TPF2's, none here.
	if first then
		if network == "Street" then
			capture.street, capture.bus_lane, capture.tram = first.template, false, "None"
		else
			capture.track, capture.catenary = first.template, false
		end
		capture.style = first.style
	end
	return capture
end

-- What a tool changed, for the log: for each added edge between two
-- existing nodes, the fields that differ from the game's edge between them
-- now (`field=old>new`), and each node configuration it adds, summed up.
-- "" when nothing reads.
-- Engine values are userdata whose fields pairs() cannot list: these are
-- read by name (build 40408's LaneConfig, LaneConnection, BaseNodeConfig,
-- TrafficLightConfig and TrafficLightState).
local KNOWN = { "speed", "width", "height", "forward", "offset", "transportModes", "segment0", "lane0",
	"segment1", "lane1", "withRoad", "withTram", "lockedLanes", "duration", "minDuration", "canSkip",
	"states", "trafficLightType", "laneConnections", "crosswalks", "trafficLightPreference",
	"trafficLightConfig", "doubleSlipSwitch", "userModifiedLaneConnections",
	"userModifiedTrafficLightStates", "x", "y", "z" }
local function ser(v, depth)
	depth = depth or 0
	if depth > 5 then return "..." end
	local t = type(v)
	if t ~= "table" and t ~= "userdata" then return tostring(v) end
	local ok, n = pcall(function() return #v end)
	if ok and type(n) == "number" and n > 0 then
		local parts = {}
		for i = 1, math.min(n, 40) do
			local okI, x = pcall(function() return v[i] end)
			parts[#parts + 1] = okI and ser(x, depth + 1) or "?"
		end
		return "[" .. table.concat(parts, ",") .. (n > 40 and ",..." or "") .. "]"
	end
	local parts = {}
	pcall(function()
		for k, x in pairs(v) do parts[#parts + 1] = tostring(k) .. "=" .. ser(x, depth + 1) end
	end)
	if #parts == 0 then
		for _, f in ipairs(KNOWN) do
			local okF, x = pcall(function() return v[f] end)
			if okF and x ~= nil then parts[#parts + 1] = f .. "=" .. ser(x, depth + 1) end
		end
	end
	if #parts == 0 then return t == "table" and "{}" or "<" .. t .. ">" end
	table.sort(parts)
	return "{" .. table.concat(parts, ",") .. "}"
end

local EDGE_FIELDS = { "type", "typeIndex", "roadTemplate", "roadStyle", "roadDevelopmentLocked",
	"edgeDecorations", "laneConfigs" }
local STREET_FIELDS = { "precedenceNode0", "precedenceNode1" }

function engine.rebuildDiff(proposal)
	local street = get(proposal, "proposal")
	if street == nil then return "" end
	local C = api.type.ComponentType
	local function current(n0, n1)
		local found
		pcall(function()
			for _, e in ipairs(api.engine.system.streetSystem.getNodeSegments(n0)) do
				local c = api.engine.getComponent(e, C.BASE_EDGE)
				if c and ((c.node0 == n0 and c.node1 == n1) or (c.node0 == n1 and c.node1 == n0)) then found = e end
			end
		end)
		return found
	end
	local out = {}
	-- The removed edges by where their ends are: a tool that removes a node
	-- and adds it again at the same place gives its edges new node ids.
	local at = {}
	for _, n in ipairs(list(get(street, "addedNodes"))) do at[get(n, "entity")] = vec3(get(get(n, "comp"), "position")) end
	for _, n in ipairs(list(get(street, "removedNodes"))) do at[get(n, "entity")] = vec3(get(get(n, "comp"), "position")) end
	local function posKey(id)
		local p = at[id] or nodePos(id)
		if p == nil then return "?" .. tostring(id) end
		return string.format("%.0f,%.0f", p[1], p[2])
	end
	local removedAt = {}
	for _, r in ipairs(list(get(street, "removedSegments"))) do
		local c = get(r, "comp")
		if c then removedAt[posKey(c.node0) .. ">" .. posKey(c.node1)] = r end
	end
	-- One lane of the first new edge, raw: how its transport modes are keyed.
	pcall(function()
		local first = list(get(street, "addedSegments"))[1]
		local lane = list(get(get(first, "comp"), "laneConfigs"))[1]
		local tm = get(lane, "transportModes")
		local keys = {}
		for k, v in pairs(tm) do keys[#keys + 1] = tostring(k) .. "=" .. tostring(v) end
		table.sort(keys)
		out[#out + 1] = "a lane's modes (" .. type(tm) .. ", #" .. tostring(#tm) .. "): " .. table.concat(keys, ",")
	end)
	for _, s in ipairs(list(get(street, "addedSegments"))) do
		local c = get(s, "comp")
		local old = c and removedAt[posKey(c.node0) .. ">" .. posKey(c.node1)]
		if old then
			local diff = {}
			local oc = get(old, "comp")
			for _, f in ipairs(EDGE_FIELDS) do
				local a, b = ser(get(oc, f)), ser(get(c, f))
				if a ~= b then diff[#diff + 1] = f .. "=" .. a .. ">" .. b end
			end
			local a, b = ser(get(old, "streetEdge")), ser(get(s, "streetEdge"))
			if a ~= b then diff[#diff + 1] = "streetEdge=" .. a .. ">" .. b end
			a, b = ser(get(old, "playerOwned")), ser(get(s, "playerOwned"))
			if a ~= b then diff[#diff + 1] = "playerOwned=" .. a .. ">" .. b end
			out[#out + 1] = "moved edge " .. posKey(c.node0) .. ">" .. posKey(c.node1) .. ": "
				.. (#diff > 0 and table.concat(diff, " ") or "same")
		end
	end
	for _, s in ipairs(list(get(street, "addedSegments"))) do
		local c = get(s, "comp")
		local n0, n1 = c and get(c, "node0"), c and get(c, "node1")
		if type(n0) == "number" and type(n1) == "number" and n0 >= 0 and n1 >= 0 then
			local e = current(n0, n1)
			local diff = {}
			if e == nil then
				diff[1] = "no edge between them now"
			else
				local old = api.engine.getComponent(e, C.BASE_EDGE)
				for _, f in ipairs(EDGE_FIELDS) do
					local a, b = ser(get(old, f)), ser(get(c, f))
					if a ~= b then diff[#diff + 1] = f .. "=" .. a .. ">" .. b end
				end
				local oldStreet = api.engine.getComponent(e, C.BASE_EDGE_STREET)
				local newStreet = get(s, "streetEdge")
				if oldStreet or newStreet then
					for _, f in ipairs(STREET_FIELDS) do
						local a, b = ser(oldStreet and get(oldStreet, f)), ser(newStreet and get(newStreet, f))
						if a ~= b then diff[#diff + 1] = f .. "=" .. a .. ">" .. b end
					end
				end
				local owner = api.engine.getComponent(e, C.PLAYER_OWNED)
				local a, b = ser(owner and get(owner, "player")), ser(get(get(s, "playerOwned"), "player"))
				if a ~= b then diff[#diff + 1] = "owner=" .. a .. ">" .. b end
			end
			out[#out + 1] = "edge " .. n0 .. ">" .. n1 .. (e and (" (" .. e .. ")") or "") .. ": "
				.. (#diff > 0 and table.concat(diff, " ") or "same")
		end
	end
	for _, nc in ipairs(list(get(street, "nodeConfigsToAdd"))) do
		out[#out + 1] = "nodeConfig " .. tostring(get(nc, "entity")) .. ": " .. ser(get(nc, "comp"))
	end
	for _, k in ipairs({ "nodeConfigsToRemove", "edgeObjectsToAdd", "edgeObjectsToRemove", "removedSegments",
		"removedNodes", "addedNodes" }) do
		local n = #list(get(street, k))
		if n > 0 then out[#out + 1] = k .. "#" .. n end
	end
	for _, k in ipairs({ "toAdd", "toRemove" }) do
		local n = #list(get(proposal, k))
		if n > 0 then out[#out + 1] = k .. "#" .. n end
	end
	return table.concat(out, "; ")
end

-- A tool's proposal in one line, for the log: nodes added (+n) and removed
-- (-n), edges added (+e) and removed (-e) with their ends, existing nodes
-- with their positions; a construction's own nodes and edges (its frozen
-- ones) marked "*"; each edge's stops and signals ([entity:type]), the
-- edge objects added (+o, what of the record reads) and removed (-o, what
-- their EDGE_OBJECT says).
function engine.describe(proposal)
	local ok, text = pcall(function()
		local street = get(proposal, "proposal")
		local out = {}
		-- The constructions' own nodes and edges.
		local frozen = {}
		for _, c in ipairs(list(get(proposal, "toAdd"))) do
			local con = get(c, "construction")
			for _, key in ipairs({ "frozenNodes", "frozenEdges" }) do
				for _, e in ipairs(list(con and get(con, key))) do frozen[e] = true end
			end
		end
		local function mark(e) return frozen[e] and "*" or "" end
		local function at(p)
			p = vec3(p)
			if p == nil or type(p[1]) ~= "number" then return "(?)" end
			return string.format("(%.1f,%.1f,%.1f)", p[1], p[2], p[3])
		end
		local function node(id)
			if type(id) == "number" and id >= 0 then return tostring(id) .. at(nodePos(id)) end
			return tostring(id)
		end
		-- An edge's stops and signals as its component lists them,
		-- [entity:EdgeObjectType,...]; nothing for none.
		local function objects(comp)
			local parts = {}
			for _, o in ipairs(list(comp and get(comp, "objects"))) do
				parts[#parts + 1] = tostring(get(o, 1)) .. ":" .. tostring(get(o, 2))
			end
			return #parts > 0 and ("[" .. table.concat(parts, ",") .. "]") or ""
		end
		-- The names a game record's metatable holds (its members, where the
		-- binding keeps them there), for a record whose fields do not read.
		local function members(v)
			local names = {}
			pcall(function()
				local mt = getmetatable(v)
				local index = type(mt) == "table" and mt.__index
				for _, t in ipairs({ mt, type(index) == "table" and index or nil }) do
					if type(t) == "table" then
						for k in pairs(t) do
							if type(k) == "string" and k:sub(1, 2) ~= "__" then names[#names + 1] = k end
						end
					end
				end
			end)
			table.sort(names)
			return table.concat(names, ",")
		end
		for _, n in ipairs(list(get(street, "addedNodes"))) do
			out[#out + 1] = "+n" .. tostring(n.entity) .. mark(n.entity) .. at(n.comp.position)
		end
		for _, n in ipairs(list(get(street, "removedNodes"))) do
			out[#out + 1] = "-n" .. tostring(n.entity) .. at(n.comp and n.comp.position)
		end
		for _, s in ipairs(list(get(street, "addedSegments"))) do
			out[#out + 1] = "+e" .. tostring(s.entity) .. mark(s.entity) .. "/" .. tostring(s.type) .. ":"
				.. node(s.comp.node0) .. mark(s.comp.node0) .. ">" .. node(s.comp.node1) .. mark(s.comp.node1)
				.. objects(s.comp)
		end
		for _, s in ipairs(list(get(street, "removedSegments"))) do
			out[#out + 1] = "-e" .. tostring(s.entity) .. ":" .. node(s.comp.node0) .. ">" .. node(s.comp.node1)
				.. objects(s.comp)
		end
		-- Stops, signals and waypoints: whatever of their fields reads.
		for _, o in ipairs(list(get(street, "edgeObjectsToAdd"))) do
			local fields = {}
			for _, key in ipairs({ "resultEntity", "category", "left", "oneWay", "playerEntity", "edgeEntity",
				"edge", "param", "model", "name" }) do
				local v = get(o, key)
				if v ~= nil then fields[#fields + 1] = key .. "=" .. tostring(v) end
			end
			local params = get(o, "params")
			if params ~= nil then fields[#fields + 1] = "params=" .. ser(params) end
			local mi = get(o, "modelInstance")
			if mi ~= nil then
				local t = get(mi, "transf")
				fields[#fields + 1] = "modelId=" .. tostring(get(mi, "modelId"))
				if t ~= nil then fields[#fields + 1] = "at=" .. at({ get(t, 13), get(t, 14), get(t, 15) }) end
			end
			local names = type(o) == "userdata" and members(o) or ""
			if names ~= "" then fields[#fields + 1] = "members=" .. names end
			out[#out + 1] = "+o{" .. table.concat(fields, " ") .. "}"
		end
		-- The objects it removes, by entity: what their EDGE_OBJECT says.
		for _, e in ipairs(list(get(street, "edgeObjectsToRemove"))) do
			local fields = {}
			local c
			pcall(function() c = api.engine.getComponent(e, api.type.ComponentType.EDGE_OBJECT) end)
			if c == nil then
				fields[1] = "no edge object"
			else
				for _, key in ipairs({ "edgeEntity", "param", "left", "oneWay", "edgeObjectConstruction" }) do
					local v = get(c, key)
					if v ~= nil then fields[#fields + 1] = key .. "=" .. tostring(v) end
				end
				local t = get(c, "transf")
				if t ~= nil then fields[#fields + 1] = "at=" .. at({ get(t, 13), get(t, 14), get(t, 15) }) end
			end
			out[#out + 1] = "-o" .. tostring(e) .. "{" .. table.concat(fields, " ") .. "}"
		end
		-- Junction settings: each node's lanes and how many a tram's.
		for _, n in ipairs(list(get(street, "nodeConfigsToAdd"))) do
			local turns, trams = 0, 0
			for _, t in ipairs(list(get(n.comp, "laneConnections"))) do
				turns = turns + 1
				if get(t, "withTram") == true then trams = trams + 1 end
			end
			out[#out + 1] = "+j" .. node(n.entity) .. "{lanes " .. turns .. " tram " .. trams .. "}"
		end
		for _, id in ipairs(list(get(street, "nodeConfigsToRemove"))) do
			out[#out + 1] = "-j" .. node(id)
		end
		for _, c in ipairs(list(get(proposal, "toAdd"))) do
			local con = get(c, "construction")
			out[#out + 1] = "+c" .. tostring(get(c, "fileName")) .. "{frozen "
				.. #list(con and get(con, "frozenNodes")) .. "n " .. #list(con and get(con, "frozenEdges")) .. "e}"
		end
		for _, c in ipairs(list(get(proposal, "toRemove"))) do
			out[#out + 1] = "-c" .. tostring(c)
		end
		return table.concat(out, " ")
	end)
	if ok then return text end
	return "unreadable: " .. tostring(text)
end

-- A stop's removal ("stops", below).
local removeStop

-- What an entity the bulldozer removes is, when it is no construction, in a
-- few words for the player and the log. An asset group (trees, rocks and
-- other assets, which the asset bulldozer takes out of their group and
-- rebuilds the rest: build 40408, construction_builder_util::
-- CreateProposalAddAsset) has words of its own; anything else is named by
-- the components it has, so a test in the game says what it was.
local KINDS = { "ASSET_GROUP", "TOWN_BUILDING", "SUBCONSTRUCTION", "INDUSTRY", "FIELD", "TOWN",
	"MODEL_INSTANCE_LIST", "BASE_EDGE", "BASE_NODE", "EDGE_OBJECT", "STATION_GROUP", "PLAYER_OWNED" }
function engine.notConstruction(entity)
	local types = get(get(api, "type"), "ComponentType")
	local has = {}
	for _, kind in ipairs(KINDS) do
		local id = types and get(types, kind)
		if id ~= nil then
			local ok, c = pcall(api.engine.getComponent, entity, id)
			if ok and c ~= nil then has[#has + 1] = kind end
		end
	end
	if has[1] == "ASSET_GROUP" then
		return "removing trees or other assets (asset group " .. tostring(entity)
			.. "), which the room does not carry yet"
	end
	return "removing something that is no construction (entity " .. tostring(entity) .. ": "
		.. (#has > 0 and table.concat(has, ", ") or "no component it knows") .. ")"
end

-- ----------------------------------------------------------------- assets
--
-- Trees and other assets stand in asset groups (ASSET_GROUP, with a
-- MODEL_INSTANCE_LIST of thin instances, a model with a position, a turn
-- about the vertical and a scale, and full ones, a model with a matrix).
-- The asset bulldozer (build 40408, UI::AssetBulldozerAction and
-- construction_builder_util::CreateProposalAddAsset, decompiled) removes
-- the group and, unless it took every asset out, adds it again without the
-- ones taken: one construction entity, at the world's origin, whose one
-- subconstruction lists the assets kept as models, the thin ones first and
-- then the full ones, each its model's file and its world matrix (a thin
-- one's built from its turn, scale and position; a full one's its own), and
-- whose desc is autoRemovable. Its file is empty (in TF3 the entity's
-- fileName only reads; it is the desc's). The room carries which assets
-- went (action::Bulldoze::Assets); every game builds the group again from
-- its own copy (tpf3mp/apply.lua).

-- Positions of one asset match within this, per axis, in metres.
engine.ASSET_TOLERANCE = 0.005

-- Whether the asset bulldozer's removals go to the room: only where the
-- hook says so (TPF3MP_TREE_BULLDOZE=1), for a trial of the replay.
function engine.treesOn()
	-- The game's GUI state has no rawget, and reading an unset global may
	-- throw there: read it as bridge.lua does.
	local ok, on = pcall(function() return tpf3mp_native.trees() end)
	return ok and on == true
end

-- A model's file, as the tool's rebuilt group names it ("::/assets/...").
local function modelFile(name)
	if type(name) ~= "string" or name == "" then return nil end
	return "::/" .. name:gsub("^::/", "")
end
engine.modelFile = modelFile

-- A matrix's 16 numbers, or nil where one does not read.
local function matrix16(t)
	if t == nil then return nil end
	local out = {}
	for i = 1, 16 do
		local v = get(t, i)
		if type(v) ~= "number" then return nil end
		out[i] = v
	end
	return out
end

-- An asset group's assets, in the order the asset bulldozer rebuilds them:
-- its thin instances, { thin = true, model =, x =, y =, z =, rot =, scale = },
-- then its full ones, { thin = false, model =, x =, y =, z =, m = its 16
-- matrix elements }; or raises.
function engine.assetsOf(group)
	local types = api.type.ComponentType
	if api.engine.getComponent(group, types.ASSET_GROUP) == nil then error("no asset group", 0) end
	local m = api.engine.getComponent(group, types.MODEL_INSTANCE_LIST)
	if m == nil then error("an asset group with no models", 0) end
	local out = {}
	local function file(id, i)
		local f = modelFile(api.res.modelRep.getName(id))
		if f == nil then error("asset " .. i .. " of the group, of model " .. tostring(id) .. ", has no file", 0) end
		return f
	end
	for _, t in ipairs(list(get(m, "thinInstances"))) do
		local i = #out + 1
		local p = vec3(get(t, "pos"))
		local rot, scale = get(t, "rot"), get(t, "scale")
		if p == nil or type(p[1]) ~= "number" or type(p[2]) ~= "number" or type(p[3]) ~= "number"
			or type(rot) ~= "number" or type(scale) ~= "number" then
			error("asset " .. i .. " of the group does not read", 0)
		end
		out[i] = { thin = true, model = file(get(t, "modelId"), i), x = p[1], y = p[2], z = p[3], rot = rot,
			scale = scale }
	end
	for _, f in ipairs(list(get(m, "fatInstances"))) do
		local i = #out + 1
		local mt = matrix16(get(f, "transf"))
		if mt == nil then error("asset " .. i .. " of the group (a full instance) does not read", 0) end
		out[i] = { thin = false, model = file(get(f, "modelId"), i), x = mt[13], y = mt[14], z = mt[15], m = mt }
	end
	return out
end

-- An asset's world matrix, the game's 16 elements (columns of four): a
-- full instance's own; a thin one's a turn of `rot` about the vertical,
-- scaled by `scale`, at its position. `mirrored` turns the other way round
-- in the matrix.
function engine.assetMatrix(a, mirrored)
	if a.m then
		local out = {}
		for i = 1, 16 do out[i] = a.m[i] end
		return out
	end
	local c, s = math.cos(a.rot) * a.scale, math.sin(a.rot) * a.scale
	if mirrored then s = -s end
	return { c, s, 0, 0, -s, c, 0, 0, 0, 0, a.scale, 0, a.x, a.y, a.z, 1 }
end

-- Whether asset `a` is the one of `model` at x, y, z.
function engine.assetAt(a, model, x, y, z)
	local tol = engine.ASSET_TOLERANCE
	return a.model == model and math.abs(a.x - x) <= tol and math.abs(a.y - y) <= tol and math.abs(a.z - z) <= tol
end

-- How many of `assets` are the one of `model` at x, y, z: more than one,
-- and the room could not say which of them went.
function engine.assetsAt(assets, model, x, y, z)
	local n = 0
	for _, a in ipairs(assets) do
		if engine.assetAt(a, model, x, y, z) then n = n + 1 end
	end
	return n
end

-- Takes from `assets` (each matched once, `used` marks them) the one of
-- `model` at x, y, z; its index, or nil.
function engine.takeAsset(assets, used, model, x, y, z)
	for i, a in ipairs(assets) do
		if not used[i] and engine.assetAt(a, model, x, y, z) then
			used[i] = true
			return i
		end
	end
	return nil
end

-- The asset bulldozer's proposal as a Bulldoze::Assets, or nil and why:
-- one asset group removed, and either nothing added (every asset of it
-- taken) or one construction entity of no file added at the world's origin,
-- whose models are the group's own assets less some, each where it stood
-- and turned as it was; nothing else.
function engine.captureAssets(proposal)
	local ok, action = pcall(function()
		local toRemove, toAdd = list(get(proposal, "toRemove")), list(get(proposal, "toAdd"))
		if #toRemove ~= 1 or #toAdd > 1 then error("taking assets out of more than one group at once", 0) end
		local street = get(proposal, "proposal")
		for _, key in ipairs({ "addedNodes", "addedSegments", "removedNodes", "removedSegments",
			"edgeObjectsToAdd", "edgeObjectsToRemove" }) do
			if #list(street and get(street, key)) > 0 then error("an asset bulldoze that changes streets too", 0) end
		end
		local group, ce = toRemove[1], toAdd[1]
		local old = engine.assetsOf(group)
		local used, pairs_ = {}, {}
		local owner
		if ce ~= nil then
			local file = get(ce, "fileName")
			if file ~= nil and tostring(file) ~= "" then error("an asset group rebuilt as " .. tostring(file), 0) end
			local t = get(ce, "transf")
			local identity = { 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1 }
			for i = 1, 16 do
				local v = get(t, i)
				if type(v) ~= "number" or math.abs(v - identity[i]) > 1e-6 then
					error("an asset group rebuilt away from the world's origin (element " .. i .. " " .. tostring(v) .. ")", 0)
				end
			end
			local con = get(ce, "construction")
			for _, sub in ipairs(list(con and get(con, "subconstructions"))) do
				for _, m in ipairs(list(get(sub, "models"))) do
					local mt = get(m, "transf")
					local model = modelFile(get(m, "id"))
					local x, y, z = get(mt, 13), get(mt, 14), get(mt, 15)
					local i = model and type(x) == "number" and type(y) == "number" and type(z) == "number"
						and engine.takeAsset(old, used, model, x, y, z)
					if not i then
						error("the rebuilt group holds " .. tostring(get(m, "id")) .. " at " .. tostring(x) .. ", "
							.. tostring(y) .. ", " .. tostring(z) .. ", which the group did not", 0)
					end
					pairs_[#pairs_ + 1] = { asset = old[i], transf = mt }
				end
			end
			if #pairs_ == 0 then error("an asset group rebuilt with nothing in it", 0) end
			owner = get(ce, "playerEntity")
		end
		local removed = {}
		for i, a in ipairs(old) do
			if not used[i] then
				-- Another asset of its model where it stood: which of them
				-- went, the room could not say.
				if engine.assetsAt(old, a.model, a.x, a.y, a.z) > 1 then
					error(string.format("two assets of %s at %.3f, %.3f, %.3f: which one went is not clear", a.model,
						a.x, a.y, a.z), 0)
				end
				removed[#removed + 1] = { model = a.model, at = { x = a.x, y = a.y, z = a.z } }
			end
		end
		if #removed == 0 then error("an asset bulldoze that removes nothing", 0) end
		if #removed > 64 then error("taking " .. #removed .. " assets at once, more than the room carries (64)", 0) end
		-- Which way round the tool turned the thin ones: every one kept must
		-- read as one of the two, the same for all. A full one keeps its own
		-- matrix.
		local ROTATION, ALL = { 1, 2, 3, 5, 6, 7, 9, 10, 11 }, {}
		for k = 1, 16 do ALL[k] = k end
		local function fits(mirrored, thin)
			for _, pr in ipairs(pairs_) do
				if pr.asset.thin == thin then
					local want = engine.assetMatrix(pr.asset, mirrored)
					for _, k in ipairs(thin and ROTATION or ALL) do
						local v = get(pr.transf, k)
						if type(v) ~= "number" or math.abs(v - want[k]) > 1e-3 then return false, pr, k, v, want[k] end
					end
				end
			end
			return true
		end
		local full, fpr, fk, fv, fwant = fits(false, false)
		if not full then
			error(string.format("the tool moves the full instance %s (element %d: %s, not %s)",
				fpr.asset.model, fk, tostring(fv), tostring(fwant)), 0)
		end
		local mirrored = false
		local plain, pr, k, v, want = fits(false, true)
		if not plain then
			if fits(true, true) then
				mirrored = true
			else
				error(string.format("the tool turns %s differently than the room would build it (element %d: %s, not %s)",
					pr.asset.model, k, tostring(v), tostring(want)), 0)
			end
		end
		local first = old[1]
		return { Bulldoze = { Assets = {
			first = { model = first.model, at = { x = first.x, y = first.y, z = first.z } },
			count = #old,
			removed = removed,
			mirrored = mirrored,
			owned = type(owner) == "number" and owner >= 0,
		} } }
	end)
	if not ok then return nil, tostring(action) end
	return action
end

-- Whether a CONSTRUCTION component is a town building's: one that lists
-- its town buildings (as capture.construction tells them).
local function isTownBuilding(c)
	return #list(c and get(c, "townBuildings")) > 0
end

-- Whether `removedObjects` (the proposal's edgeObjectsToRemove) are exactly
-- the objects `segments` carry, each a SIGNAL (the engine's type for signals
-- and waypoints alike) listed by one removed edge once: the signals the game
-- removes with the edges under them, and no stop or other object.
local function signalsOfRemovedEdges(segments, removedObjects)
	local types = api.type.enum and api.type.enum.EdgeObjectType
	local signal = types and types.SIGNAL
	if type(signal) ~= "number" or #removedObjects == 0 then return false end
	local removed, occurrences = {}, {}
	for i = 1, #removedObjects do
		local row = removedObjects[i]
		local entity = type(row) == "number" and row or get(row, "entity")
		if type(entity) ~= "number" or entity ~= math.floor(entity) or entity <= 0 or removed[entity] then
			return false
		end
		removed[entity] = true
	end
	for i = 1, #segments do
		local objects = list(get(get(segments[i], "comp"), "objects"))
		for k = 1, #objects do
			local pair = objects[k]
			local entity, kind = get(pair, 1), get(pair, 2)
			if kind ~= signal or not removed[entity] then
				return false
			end
			occurrences[entity] = (occurrences[entity] or 0) + 1
			if occurrences[entity] ~= 1 then return false end
		end
	end
	for entity in pairs(removed) do
		if occurrences[entity] ~= 1 then return false end
	end
	return true
end

-- Whether every edge a bulldozer's street proposal adds joins two of the
-- edges it removes where they meet at a node it removes, between their
-- other ends. A street's branch removed, or a track's stub ending on a
-- street, takes the junction or crossing node with it, and the bulldozer
-- joins the two street edges left there into one (a room, build 40420,
-- 2026-10-16: [-n39316 +e-1/0:159842>76648 -e39330:39316>39317
-- -e39328:159842>39316 -e39329:39316>76648], e39330 the track's stub); a
-- track's switch stays. The game's own removal of the branch alone
-- (makeSegmentsRemoveProposal) joins nothing and was refused there
-- ("Construction Not Possible"), so the join travels with it.
local function joinsOnly(street)
	local gone = {}
	for _, n in ipairs(list(get(street, "removedNodes"))) do
		local e = get(n, "entity")
		if type(e) ~= "number" then return false end
		gone[e] = true
	end
	local removed, used, joinedAt = list(get(street, "removedSegments")), {}, {}
	-- The end of a removed edge opposite node n, if it touches n.
	local function across(seg, n)
		local c = get(seg, "comp")
		if get(c, "node0") == n then return get(c, "node1") end
		if get(c, "node1") == n then return get(c, "node0") end
		return nil
	end
	-- A removed track is the crossing's bulldozed stub only when it touches
	-- exactly one node already accounted for by a street join, both ends are
	-- removed, and its other end has no other track in the live network map.
	local function bulldozedStub(seg)
		if networkOf(seg) ~= "Track" then return false end
		local entity, comp = get(seg, "entity"), get(seg, "comp")
		local a, b = get(comp, "node0"), get(comp, "node1")
		if type(entity) ~= "number" or entity <= 0 or entity % 1 ~= 0
			or type(a) ~= "number" or a % 1 ~= 0 or type(b) ~= "number" or b % 1 ~= 0
			or a == b or not gone[a] or not gone[b] then return false end
		local junction
		if joinedAt[a] and not joinedAt[b] then junction = a
		elseif joinedAt[b] and not joinedAt[a] then junction = b
		else return false end
		local tip = junction == a and b or a
		local atJunction, atTip = nodeEdges(junction, "Track"), nodeEdges(tip, "Track")
		if atJunction == nil or atTip == nil then return false end
		atJunction, atTip = list(atJunction), list(atTip)
		local present = false
		for _, edge in ipairs(atJunction) do
			if edge == entity then present = true break end
		end
		return present and #atTip == 1 and atTip[1] == entity
	end
	for _, seg in ipairs(list(get(street, "addedSegments"))) do
		local comp = get(seg, "comp")
		local a, b = get(comp, "node0"), get(comp, "node1")
		local found, foundAt
		for i, s in ipairs(removed) do
			for _, n in ipairs({ get(get(s, "comp"), "node0"), get(get(s, "comp"), "node1") }) do
				if gone[n] and n ~= a and n ~= b and across(s, n) == a then
					for j, t in ipairs(removed) do
						if j ~= i and across(t, n) == b then
							if found ~= nil then return false end
							found = { i, j }
							foundAt = n
						end
					end
				end
			end
		end
		if found == nil or used[found[1]] or used[found[2]] then return false end
		for _, k in ipairs(found) do
			if get(removed[k], "type") ~= get(seg, "type") then return false end
		end
		used[found[1]], used[found[2]] = true, true
		joinedAt[foundAt] = true
	end
	for i, seg in ipairs(removed) do
		if not used[i] and not bulldozedStub(seg) then return false end
	end
	return true
end

-- A stock airport demolition rebuilds its runway/taxiway signals as part of
-- removing the construction. Permit that batch only when the proposal removes
-- exactly every edge frozen into this construction and every removed object is
-- a unique SIGNAL carried by one of those edges; external edges, stops, and
-- unreadable carriers remain refused.
local function airportRemovalSignals(construction, segments, removedObjects)
	local file = get(construction, "fileName")
	if file ~= "::/stations/air/airfield.con" and file ~= "::/stations/air/airport.con" then return false end
	local frozen, frozenList = {}, list(get(construction, "frozenEdges"))
	local frozenCount = #frozenList
	if frozenCount == 0 or #segments ~= frozenCount then return false end
	for i = 1, frozenCount do
		local edge = get(frozenList, i)
		if type(edge) ~= "number" or edge ~= math.floor(edge) or edge <= 0 or frozen[edge] then return false end
		frozen[edge] = true
	end
	local removedEdges = {}
	for i = 1, #segments do
		local edge = get(segments[i], "entity")
		if type(edge) ~= "number" or edge ~= math.floor(edge) or not frozen[edge] or removedEdges[edge] then
			return false
		end
		removedEdges[edge] = true
	end
	for edge in pairs(frozen) do
		if not removedEdges[edge] then return false end
	end
	return signalsOfRemovedEdges(segments, removedObjects)
end

-- Tracks the bulldozer removes with the signals on them (build 40420,
-- 2026-10-16: a track with one signal, "1 Signal Will Be Removed"): no
-- construction, no object added, every removed edge a track, and the removed
-- objects exactly its signals. Each game finds the same signals on the same
-- tracks again before removing them (apply.lua, HANDLERS.Bulldoze).
local function trackRemovalSignals(toRemove, segments, objectsToAdd, objectsToRemove)
	if #toRemove > 0 or #objectsToAdd > 0 or #segments == 0 then return false end
	for i = 1, #segments do
		local ok, network = pcall(networkOf, segments[i])
		if not ok or network ~= "Track" then return false end
	end
	return signalsOfRemovedEdges(segments, objectsToRemove)
end

-- The bulldozer's proposal as a Bulldoze action (tpf3mp_proto
-- action::Bulldoze): one construction, by its file and position, whose own
-- entrance edge and node the game removes with it; or edges of one network,
-- by their ends, with the nodes they leave on their own and the town
-- buildings the game removes with them, each by its file and position
-- (build 40408, the bulldozer hovered and clicked: a street of a town
-- proposed with the buildings along it, street_util::FinishProposal). false
-- for a proposal of nothing; nil and why the room cannot carry it.
function engine.bulldoze(proposal)
	local ok, action = pcall(function()
		local street = get(proposal, "proposal")
		if street == nil then error("a proposal with no street proposal", 0) end
		-- A stop removed: its edge rebuilt without it, nothing else (no
		-- node goes; a junction removed with a branch does, below).
		if #list(get(proposal, "toAdd")) == 0 and #list(get(proposal, "toRemove")) == 0
			and #list(get(street, "addedSegments")) == 1 and #list(get(street, "addedNodes")) == 0
			and #list(get(street, "removedNodes")) == 0 then
			return removeStop(street)
		end
		-- A branch removed where the bulldozer joins the two edges left at
		-- its junction: carried as the build it is, a BuildRoad or
		-- BuildTrack that removes the edges and nodes and adds the joining
		-- edge, as a road modifier's build is (engine.captureModify).
		if #list(get(proposal, "toAdd")) == 0 and #list(get(street, "addedNodes")) == 0
			and #list(get(street, "addedSegments")) > 0 and joinsOnly(street) then
			if #list(get(proposal, "toRemove")) > 0 then
				error("removing a street with town buildings where the bulldozer joins the streets left", 0)
			end
			local capture = engine.fromProposal(proposal, nil, false)
			local built, why = roads.capture(capture, engine.world())
			if built == nil then error(tostring(why), 0) end
			return built
		end
		if #list(get(proposal, "toAdd")) > 0 or #list(get(street, "addedSegments")) > 0
			or #list(get(street, "addedNodes")) > 0 then
			error("a bulldozer proposal that builds", 0)
		end
		local toRemove = list(get(proposal, "toRemove"))
		local segments = list(get(street, "removedSegments"))
		local objectsToAdd = list(get(street, "edgeObjectsToAdd"))
		local objectsToRemove = list(get(street, "edgeObjectsToRemove"))
		local airportSignals, trackSignals = false, false
		for _, name in ipairs({ "edgeObjectsToAdd", "edgeObjectsToRemove" }) do
			local v = get(street, name)
			if v ~= nil and #list(v) > 0 then
				if name == "edgeObjectsToRemove" and #objectsToAdd == 0 and #toRemove == 1 then
					local construction = api.engine.getComponent(toRemove[1], api.type.ComponentType.CONSTRUCTION)
					if construction ~= nil and not isTownBuilding(construction) then
						airportSignals = airportRemovalSignals(construction, segments, objectsToRemove)
					end
				end
				if name == "edgeObjectsToRemove" then
					trackSignals = trackRemovalSignals(toRemove, segments, objectsToAdd, objectsToRemove)
				end
				if not airportSignals and not trackSignals then error("removing a stop or signal", 0) end
			end
		end
		-- The constructions it removes: town buildings, and any other.
		local buildings, others = {}, 0
		for _, e in ipairs(toRemove) do
			local c = api.engine.getComponent(e, api.type.ComponentType.CONSTRUCTION)
			if c == nil then error(engine.notConstruction(e), 0) end
			if isTownBuilding(c) then
				local t = c.transf
				buildings[#buildings + 1] = { file = resName(c.fileName, "a town building's file"),
					at = { x = t[13], y = t[14], z = t[15] } }
			else
				others = others + 1
			end
		end
		-- One construction, with what is its own (a depot and its entrance
		-- edge), or a town building alone: the construction by its file and
		-- place, removed as the game removes it (createProposalRemove).
		if #segments == 0 or others > 0 then
			if #toRemove > 1 then error("removing more than one construction at once", 0) end
			if #toRemove == 1 then
				local c = api.engine.getComponent(toRemove[1], api.type.ComponentType.CONSTRUCTION)
				local t = c.transf
				return { Bulldoze = { Construction = {
					file = resName(c.fileName, "a construction's file"),
					at = { x = t[13], y = t[14], z = t[15] },
				} } }
			end
			return false
		end
		-- Streets, and the town buildings along them the game removes too.
		if #buildings > 64 then
			error("removing " .. #buildings .. " town buildings at once, more than the room carries (64)", 0)
		end
		local network, edges = nil, {}
		for k, seg in ipairs(segments) do
			local n = networkOf(seg)
			if network == nil then
				network = n
			elseif n ~= network then
				error("removing streets and tracks at once", 0)
			end
			-- Signals go with their tracks, each proven above to be one the
			-- game removes with them; any other object refuses it.
			if not trackSignals and #objectEntities(seg.comp) > 0 then
				error("removing an edge with a stop or signal on it", 0)
			end
			local a, b = nodePos(seg.comp.node0), nodePos(seg.comp.node1)
			if a == nil or b == nil then error("removed edge " .. k .. " has no position here", 0) end
			edges[k] = { a = { x = a[1], y = a[2], z = a[3] }, b = { x = b[1], y = b[2], z = b[3] } }
		end
		return { Bulldoze = { Edges = { network = network, edges = edges, buildings = buildings } } }
	end)
	if not ok then return nil, tostring(action) end
	return action
end

-- ------------------------------------------------------------------ stops
--
-- The stop tool (streetTerminalBuilder) and the bulldozer over a stop hand
-- game scripts the shape the game's own mission scripts check a stop by
-- (mission_task_build_construction_util.tl, checkStop, build 40408): one
-- existing edge removed, and the same edge added again (a new entity between
-- the same two nodes) whose `objects` list the edge's stops and signals as
-- { entity, EdgeObjectType }, as many as `edgeObjectsToAdd`. An object the
-- edge had keeps its entity there (re-parented, its station group and lines
-- kept; TPF2's tool did so, docs/BUILDING.md), so the new stop is the one
-- entity the old edge did not list, and a removed one the entity the new
-- edge no longer lists. INFERRED: the order of `objects` is the order of
-- `edgeObjectsToAdd`, as the mission scripts' checks take it.

-- The one existing edge a stop proposal rebuilds: its removed and added
-- records, and its network; or raises.
local function rebuiltEdge(street)
	if #list(get(street, "addedNodes")) > 0 or #list(get(street, "removedNodes")) > 0 then
		error("a stop build that adds or removes nodes", 0)
	end
	local added, removed = list(get(street, "addedSegments")), list(get(street, "removedSegments"))
	if #added ~= 1 or #removed ~= 1 then error("a stop build of " .. #removed .. " edges", 0) end
	local old, new = get(removed[1], "comp"), get(added[1], "comp")
	if old == nil or new == nil then error("an edge with no component", 0) end
	if get(old, "node0") ~= get(new, "node0") or get(old, "node1") ~= get(new, "node1") then
		error("a stop build that moves its edge", 0)
	end
	local network = networkOf(removed[1])
	if networkOf(added[1]) ~= network then error("a stop build that changes its edge's network", 0) end
	return old, new, network
end

-- The objects an edge lists, as { entity, type } pairs, by entity.
local function objectsOf(comp)
	local out, byEntity = {}, {}
	for _, o in ipairs(list(get(comp, "objects"))) do
		local entity, kind = get(o, 1), get(o, 2)
		if type(entity) ~= "number" then error("an edge object it cannot read", 0) end
		out[#out + 1] = { entity, kind }
		byEntity[entity] = kind
	end
	return out, byEntity
end

-- The edge between an existing edge's two nodes, as the schema names it
-- (action::EdgeRef), and its geometry for geom.lua; or raises.
local function edgeRef(comp, network)
	local a, b = nodePos(get(comp, "node0")), nodePos(get(comp, "node1"))
	if a == nil or b == nil then error("the stop's edge has no position here", 0) end
	local ta, tb = vec3(get(comp, "tangent0")), vec3(get(comp, "tangent1"))
	if ta == nil or tb == nil or type(ta[1]) ~= "number" or type(tb[1]) ~= "number" then
		error("the stop's edge has no tangents", 0)
	end
	return { network = network, ends = { a = { x = a[1], y = a[2], z = a[3] }, b = { x = b[1], y = b[2], z = b[3] } } },
		{ a = a, b = b, ta = ta, tb = tb }
end

-- A stop placed with the stop tool, as a PlaceStop action (tpf3mp_proto
-- action::PlaceStop): the edge by its ends, the place on its centreline,
-- the engine's `left`, the edge's direction there and the stop's
-- construction. Transport Fever 3 builds a stop as a construction
-- ("stations/street/small_stops/small_new.con") and its tool's proposal
-- does not name it (build 40408: the edge objects have no model), so
-- `noted` is the one the construction menu gave the tool
-- (tpf3mp/capture.lua, capture.stop); a proposal whose edge object has a
-- model names it itself. A two-sided stop is one click that adds an object
-- on each side. false for a proposal of nothing; nil and why the room
-- cannot carry it.
-- Signals and waypoints go the same way, on a track (category 2 and 1, the
-- engine's SIGNAL), one at a time, `oneWay` as the tool had it, and with
-- the settings the tool builds them with (`params`, the schema's list:
-- capture.stop reads them off the tool; nil where it could not, which
-- refuses the signal rather than build it with the construction's
-- defaults: a mod's settings on it, Auto Signals' spacing, would be lost).
local OBJECT_KINDS = { [0] = "Stop", [1] = "Waypoint", [2] = "Signal" }
function engine.placeStop(proposal, noted, oneWay, params, paramsWhy)
	local ok, action = pcall(function()
		local street = get(proposal, "proposal")
		if street == nil then error("a proposal with no street proposal", 0) end
		if #list(get(proposal, "toAdd")) > 0 or #list(get(proposal, "toRemove")) > 0 then
			error("a stop build with constructions", 0)
		end
		local toAdd = list(get(street, "edgeObjectsToAdd"))
		if #toAdd == 0 and #list(get(street, "addedSegments")) == 0 and #list(get(street, "removedSegments")) == 0 then
			return false
		end
		local old, new, network = rebuiltEdge(street)
		local _, had = objectsOf(old)
		local now, has = objectsOf(new)
		-- A stop dropped where one stood replaces it, and the game moves its
		-- lines to the new one, which a replay cannot say (docs/BUILDING.md).
		for entity in pairs(had) do
			if has[entity] == nil then error("a stop that replaces another", 0) end
		end
		local added = {}
		for k, o in ipairs(now) do
			if had[o[1]] == nil then added[#added + 1] = k end
		end
		if #added == 0 then return false end
		if #added > 2 then error("more than two stops at once", 0) end
		-- The tool's record of each new object: its edgeObjectsToAdd lists
		-- every object of the rebuilt edge, in the edge's order (the stop
		-- tool), or the new ones alone (the signal tool on a track with
		-- signals on it already: build 40408, 2026-10-06); any other count
		-- cannot be paired.
		local record = {}
		if #toAdd == #now then
			for _, k in ipairs(added) do record[k] = toAdd[k] end
		elseif #toAdd == #added then
			for i, k in ipairs(added) do record[k] = toAdd[i] end
		else
			error("a stop build whose objects it cannot pair", 0)
		end
		local types = enum("EdgeObjectType")
		local kind = OBJECT_KINDS[get(record[added[1]], "category")]
		if kind == nil then error("an edge object of category " .. tostring(get(record[added[1]], "category")), 0) end
		if kind ~= "Stop" and #added > 1 then error("more than one signal at once", 0) end
		for _, k in ipairs(added) do
			local eo = record[k]
			if OBJECT_KINDS[get(eo, "category")] ~= kind then error("a stop and a signal at once", 0) end
			if kind == "Stop" then
				-- INFERRED: the engine lists a stop it calls left as STOP_LEFT.
				if now[k][2] ~= (get(eo, "left") == true and types.STOP_LEFT or types.STOP_RIGHT) then
					error("a stop whose side the room cannot say", 0)
				end
			elseif now[k][2] ~= types.SIGNAL then
				error("a signal the engine lists as no signal", 0)
			end
		end
		if kind ~= "Stop" and type(params) ~= "table" then
			error("a signal whose settings the room cannot read" .. (paramsWhy and (": " .. tostring(paramsWhy)) or ""), 0)
		end
		local twoSided = #added == 2
		if twoSided and (get(record[added[1]], "left") == true) == (get(record[added[2]], "left") == true) then
			error("two stops on one side", 0)
		end
		local index = added[1]
		local eo = record[index]
		local left = get(eo, "left") == true
		-- The model and place of the first of its objects that has them.
		local instance
		for _, k in ipairs(added) do
			instance = instance or get(record[k], "modelInstance")
		end
		local model
		if instance ~= nil then
			pcall(function() model = api.res.modelRep.getName(get(instance, "modelId")) end)
		end
		if type(model) ~= "string" or model == "" then model = noted end
		model = resName(model, "the stop's construction (the stop tool's, noted by the GUI)")
		local ref, curve = edgeRef(old, network)
		-- Where along the edge: the proposal's own parameter where it has
		-- one, else the point of the centreline nearest the stop's model,
		-- else nearest the cursor's viewing ray (build 40408's proposal has
		-- neither). A terrain point alone is offset behind elevated track.
		-- Every game builds it
		-- where this one says.
		local u = get(eo, "param")
		if type(u) ~= "number" or u < 0 or u > 1 then
			local t = instance and get(instance, "transf")
			local x, y = t and get(t, 13), t and get(t, 14)
			if type(x) ~= "number" or type(y) ~= "number" then
				pcall(function()
					if api.gui.mouse.hasTerrainPosition() then
						local p = api.gui.mouse.getTerrainPosition()
						x, y = p.x, p.y
						local eye = api.gui.camera and api.gui.camera.getEye()
						if eye then
							u = geom.parameterOnRay(curve.a, curve.ta, curve.b, curve.tb,
								{eye.x, eye.y, eye.z}, {p.x, p.y, p.z})
						end
					end
				end)
			end
			if type(x) ~= "number" or type(y) ~= "number" then error("a stop with no place", 0) end
			if type(u) ~= "number" or u < 0 or u > 1 then
				u = geom.parameterAt(curve.a, curve.ta, curve.b, curve.tb, x, y)
			end
		end
		local at = geom.hermitePos(curve.a, curve.ta, curve.b, curve.tb, u)
		local d = geom.hermiteTangent(curve.a, curve.ta, curve.b, curve.tb, u)
		local len = math.sqrt(d[1] * d[1] + d[2] * d[2] + d[3] * d[3])
		if len == 0 then error("the stop's edge has no direction there", 0) end
		-- The name the tool gave it (Proposal.EdgeObject.name, bound to game
		-- scripts on build 40408: street_util::MakeEdgeObjectName, a street
		-- name from the town's name list not yet taken, else "Stop #n"),
		-- the first new object's that has one; every game builds it so.
		-- Left out where it would not fit the schema's 64 bytes: every game
		-- then names it as the mod does (tpf3mp/apply.lua).
		local name
		for _, k in ipairs(added) do
			local n = get(record[k], "name")
			if name == nil and type(n) == "string" and n ~= "" and #n <= 64 then name = n end
		end
		return { PlaceStop = {
			edge = ref,
			name = name,
			at = { x = at[1], y = at[2], z = at[3] },
			left = left,
			direction = { x = d[1] / len, y = d[2] / len, z = d[3] / len },
			model = model,
			two_sided = twoSided,
			object = kind,
			one_way = kind ~= "Stop" and oneWay == true,
			params = kind ~= "Stop" and params or {},
		} }
	end)
	if not ok then return nil, tostring(action) end
	return action
end

-- The bulldozer over a stop: the edge rebuilt without it. A Bulldoze of the
-- edge object (action::Bulldoze::EdgeObject): the edge by its ends, where
-- the stop stands and its construction, as its EDGE_OBJECT component says;
-- or raises. INFERRED: the bulldozer proposes a stop's removal in the stop
-- tool's shape, less the stop (as TPF2's did). Over a two-sided stop it
-- takes both (build 40420, 2026-10-16: refused in a room before): a
-- Bulldoze of both (action::Bulldoze::EdgeObjects), only for two stops of
-- one construction, one on each side.
function removeStop(street)
	local old, new, network = rebuiltEdge(street)
	local had = objectsOf(old)
	local _, has = objectsOf(new)
	local gone = {}
	for _, o in ipairs(had) do
		if has[o[1]] == nil then gone[#gone + 1] = o end
	end
	if #gone == 0 or #had ~= #list(get(new, "objects")) + #gone then
		error("a bulldozer proposal that rebuilds an edge", 0)
	end
	if #gone > 2 then error("removing more than one stop at once", 0) end
	local objects = {}
	for i, o in ipairs(gone) do
		local c = api.engine.getComponent(o[1], api.type.ComponentType.EDGE_OBJECT)
		local t = c and get(c, "transf")
		local x, y, z = get(t, 13), get(t, 14), get(t, 15)
		if type(x) ~= "number" or type(y) ~= "number" or type(z) ~= "number" then
			error("a stop with no place", 0)
		end
		objects[i] = {
			at = { x = x, y = y, z = z },
			model = resName(get(c, "edgeObjectConstruction"), "the stop's construction"),
		}
	end
	local ref, curve = edgeRef(old, network)
	if #gone == 1 then
		return { Bulldoze = { EdgeObject = { edge = ref, at = objects[1].at, model = objects[1].model } } }
	end
	local types = enum("EdgeObjectType")
	local sides = { [types.STOP_LEFT] = "left", [types.STOP_RIGHT] = "right" }
	if gone[1][1] == gone[2][1] or sides[gone[1][2]] == nil or sides[gone[2][2]] == nil
		or gone[1][2] == gone[2][2]
		or objects[1].model ~= objects[2].model then
		error("removing more than one stop at once", 0)
	end
	local function stationGroup(entity)
		local ok, group = pcall(function()
			return api.engine.system.stationGroupSystem.getStationGroup(entity)
		end)
		if not ok or type(group) ~= "number" or group < 0 then
			error("a two-sided stop without a readable station group", 0)
		end
		local componentType = api.type.ComponentType.STATION_GROUP
		if type(componentType) ~= "number" then
			error("the station group component cannot be read", 0)
		end
		local component = api.engine.getComponent(group, componentType)
		if component == nil then error("a two-sided stop with no station group component", 0) end
		local count = 0
		for _, member in ipairs(list(get(component, "stations"))) do
			if member == entity then count = count + 1 end
		end
		if count ~= 1 then error("a two-sided stop not listed once in its station group", 0) end
		return group
	end
	if stationGroup(gone[1][1]) ~= stationGroup(gone[2][1]) then
		error("the two stops are not one two-sided stop", 0)
	end
	local function center(at)
		local u = geom.parameterAt(curve.a, curve.ta, curve.b, curve.tb, at.x, at.y)
		return geom.hermitePos(curve.a, curve.ta, curve.b, curve.tb, u)
	end
	local a, b = center(objects[1].at), center(objects[2].at)
	local dx, dy, dz = a[1] - b[1], a[2] - b[2], a[3] - b[3]
	if dx * dx + dy * dy + dz * dz > 0.25 then
		error("the two stops are at different places on the edge", 0)
	end
	return { Bulldoze = { EdgeObjects = { edge = ref, objects = objects } } }
end

-- A stop's or signal's construction parameters (a table of key = value, as
-- `EdgeObjectBuilder.params` and `EdgeObject.params` hold them) as the
-- schema's flat list (action::Param), sorted by key; or raises. Only plain
-- keys with numbers or booleans, as the signal tools set them: anything else
-- is a setting the room cannot carry, never one dropped. A key may hold a
-- dot, as a mod's do (Signal Distance's "signal_distance.local"): it is one
-- key, never a path (apply.lua, objectParams).
engine.MAX_OBJECT_PARAMS = 32
function engine.flatParams(t)
	if t == nil then return {} end
	local keys, values = {}, {}
	local ok, why = pcall(function()
		for k, v in pairs(t) do
			keys[#keys + 1] = k
			values[k] = v
		end
	end)
	if not ok then error("settings it cannot read: " .. tostring(why), 0) end
	for _, k in ipairs(keys) do
		if type(k) ~= "string" or not k:match("^[%a_][%w_%.]*$") or #k > 128 then
			error("a setting named " .. tostring(k), 0)
		end
	end
	table.sort(keys)
	if #keys > engine.MAX_OBJECT_PARAMS then error("more than " .. engine.MAX_OBJECT_PARAMS .. " settings", 0) end
	local out = {}
	for _, k in ipairs(keys) do
		local v = values[k]
		if type(v) == "number" and v == v and v ~= math.huge and v ~= -math.huge then
			if v == math.floor(v) then out[#out + 1] = { key = k, value = { Int = v } }
			else out[#out + 1] = { key = k, value = { Fixed = v } } end
		elseif type(v) == "boolean" then
			out[#out + 1] = { key = k, value = { Bool = v } }
		else
			error("setting " .. k .. " is a " .. type(v), 0)
		end
	end
	return out
end

-- The entity a SimpleProposal gives the k-th of its edgeObjectsToAdd (from
-- 1): -400000000, then down (build 40408: con_util_entity_index.h; the
-- stop tool's proposals; Auto Signals counts its new signals so across all
-- its edges).
local NEW_EDGE_OBJECT = -400000000

-- Whether two of the game's vectors are the same within a millimetre.
local function sameVec(a, b)
	a, b = vec3(a), vec3(b)
	if a == nil or b == nil then return false end
	for i = 1, 3 do
		if type(a[i]) ~= "number" or type(b[i]) ~= "number" or math.abs(a[i] - b[i]) > 0.001 then return false end
	end
	return true
end

-- PlaceSignals replays only signal objects. Refuse a script proposal that
-- changes any other track property, or the replicas would silently keep
-- their old values while the sending game applied the whole proposal.
local function sameTrackApartFromSignals(was, new)
	local function readTrackField(c, name)
		local ok, value = pcall(function() return c[name] end)
		if not ok then error("a signal build whose track fields it cannot read", 0) end
		return value
	end
	for _, field in ipairs({ "roadStyle", "roadDevelopmentLocked" }) do
		if readTrackField(was, field) ~= readTrackField(new, field) then return false end
	end
	local oldDecorations, newDecorations = list(readTrackField(was, "edgeDecorations")), list(readTrackField(new, "edgeDecorations"))
	if #oldDecorations ~= #newDecorations then return false end
	for i, old in ipairs(oldDecorations) do
		local fresh = newDecorations[i]
		if get(old, 1) ~= get(fresh, 1) or get(old, 2) ~= get(fresh, 2) then return false end
	end
	readTrackField(was, "laneConfigs")
	readTrackField(new, "laneConfigs")
	local oldLanes, newLanes = lanesOf(was), lanesOf(new)
	if #oldLanes ~= #newLanes then return false end
	for i, old in ipairs(oldLanes) do
		local fresh = newLanes[i]
		for _, field in ipairs({ "speed", "width", "height", "offset", "forward", "modes" }) do
			if old[field] ~= fresh[field] then return false end
		end
	end
	return true
end

-- A script's build that rebuilds existing tracks in place with signals
-- added to them or removed from them, and nothing else, as Auto Signals
-- sends one after its player's signal (a SimpleProposal: the edges by
-- entity in edgesToRemove, each again in edgesToAdd as a copy of its
-- BASE_EDGE whose objects keep the others by entity and name each new
-- signal by its place in edgeObjectsToAdd; the signals it replaces in
-- edgeObjectsToRemove). A PlaceSignals action (tpf3mp_proto
-- action::PlaceSignals): each edge by its ends, node 0 first, each new
-- signal's place along it from node 0 and side, each removed one's place
-- and construction; the new signals' construction, one-way and settings
-- once for all of them. false for a build of nothing; nil and why for any
-- other build, which the room cannot carry (docs/HOOKS.md, "Scripts'
-- follow-up builds").
function engine.placeSignals(simple)
	local ok, action = pcall(function()
		local street = get(simple, "streetProposal")
		if street == nil then error("a signal build with no street part", 0) end
		for _, name in ipairs({ "constructionsToAdd", "constructionsToRemove" }) do
			if #list(get(simple, name)) > 0 then error("a signal build with constructions", 0) end
		end
		for _, name in ipairs({ "nodesToAdd", "nodesToRemove", "nodeConfigsToAdd", "nodeConfigsToRemove" }) do
			if #list(get(street, name)) > 0 then error("a signal build that changes nodes or their junctions", 0) end
		end
		local crossings = get(street, "node2rcType")
		if crossings ~= nil then
			local any = false
			local read = pcall(function() for _ in pairs(crossings) do any = true end end)
			if not read or any then error("a signal build that changes level crossings", 0) end
		end
		local removes, adds = list(get(street, "edgesToRemove")), list(get(street, "edgesToAdd"))
		local objectAdds = list(get(street, "edgeObjectsToAdd"))
		local objectRemoves = list(get(street, "edgeObjectsToRemove"))
		if #removes == 0 and #adds == 0 and #objectAdds == 0 and #objectRemoves == 0 then return false end
		if #removes ~= #adds then error("a signal build that does not rebuild its edges one for one", 0) end
		local C = api.type.ComponentType
		local types = enum("EdgeObjectType")
		-- The edges it removes, as they stand.
		local old = {}
		for _, id in ipairs(removes) do
			if type(id) ~= "number" or id < 0 or old[id] then error("a signal build that removes an edge twice", 0) end
			local c = api.engine.getComponent(id, C.BASE_EDGE)
			if c == nil then error("a signal build that removes an edge that is not there", 0) end
			old[id] = c
		end
		local removing = {}
		for _, e in ipairs(objectRemoves) do
			if type(e) ~= "number" or e < 0 or removing[e] then error("a signal build that removes an object twice", 0) end
			removing[e] = true
		end
		local used, paired, segOf = {}, {}, {}
		for i, seg in ipairs(adds) do
			local entity = get(seg, "entity")
			if type(entity) ~= "number" or entity >= 0 or segOf[entity] then error("a signal build whose new edges it cannot tell apart", 0) end
			segOf[entity] = i
		end
		local model, oneWay, params, settings
		local edges = {}
		for _, seg in ipairs(adds) do
			if networkOf(seg) ~= "Track" then error("a signal build on a street", 0) end
			local new = get(seg, "comp")
			if new == nil then error("an edge with no component", 0) end
			-- The one removed edge between the same two nodes.
			local match
			for _, id in ipairs(removes) do
				local c = old[id]
				if not paired[id] and get(c, "node0") == get(new, "node0") and get(c, "node1") == get(new, "node1") then
					if match ~= nil then error("a signal build of two edges between the same nodes", 0) end
					match = id
				end
			end
			if match == nil then error("a signal build that moves an edge", 0) end
			paired[match] = true
			local was = old[match]
			local oldOwner = api.engine.getComponent(match, C.PLAYER_OWNED)
			if get(oldOwner, "player") ~= get(get(seg, "playerOwned"), "player") then
				error("a signal build that changes its track owner", 0)
			end
			if not sameVec(get(was, "tangent0"), get(new, "tangent0")) or not sameVec(get(was, "tangent1"), get(new, "tangent1"))
				or get(was, "type") ~= get(new, "type") or get(was, "typeIndex") ~= get(new, "typeIndex")
				or get(was, "roadTemplate") ~= get(new, "roadTemplate")
				or not sameTrackApartFromSignals(was, new) then
				error("a signal build that changes its track", 0)
			end
			local wasDistance, newDistance = get(was, "distance"), get(new, "distance")
			if type(wasDistance) == "number" and (type(newDistance) ~= "number" or math.abs(wasDistance - newDistance) > 0.001) then
				error("a signal build that changes its track", 0)
			end
			local had = {}
			for _, o in ipairs(list(get(was, "objects"))) do had[get(o, 1)] = get(o, 2) end
			local ref = edgeRef(was, "Track")
			local entry = { edge = ref.ends, add = {}, remove = {} }
			local kept = {}
			for _, o in ipairs(list(get(new, "objects"))) do
				local e, kind = get(o, 1), get(o, 2)
				if type(e) ~= "number" then error("an edge object it cannot read", 0) end
				if e >= 0 then
					if had[e] == nil then error("a signal build that moves an object from another edge", 0) end
					if had[e] ~= kind then error("a signal build that changes an object's kind", 0) end
					if removing[e] then error("a signal build that keeps an object it removes", 0) end
					if kept[e] then error("a signal build that lists an object twice", 0) end
					kept[e] = true
				else
					if kind ~= types.SIGNAL then error("a script's build of a new stop", 0) end
					local k = NEW_EDGE_OBJECT - e + 1
					local eo = objectAdds[k]
					if eo == nil or used[k] then error("a new signal with no record of its own", 0) end
					used[k] = true
					if segOf[get(eo, "edgeEntity")] == nil or get(eo, "edgeEntity") ~= get(seg, "entity") then
						error("a new signal recorded on another edge", 0)
					end
					local u = get(eo, "param")
					if type(u) ~= "number" or u ~= u or u < 0 or u > 1 then error("a new signal with no place on its edge", 0) end
					local m = resName(get(eo, "model"), "the new signal's construction")
					local w = get(eo, "oneWay") == true
					local p = engine.flatParams(get(eo, "params"))
					local key = {}
					for _, s in ipairs(p) do key[#key + 1] = s.key .. "=" .. tostring(s.value.Int or s.value.Fixed or s.value.Bool) end
					key = table.concat(key, ",")
					if model == nil then
						model, oneWay, params, settings = m, w, p, key
					elseif m ~= model or w ~= oneWay or key ~= settings then
						error("signals of more than one kind at once", 0)
					end
					entry.add[#entry.add + 1] = { at = u, left = get(eo, "left") == true }
				end
			end
			for e, kind in pairs(had) do
				if not kept[e] then
					if not removing[e] then error("a signal build that drops an object without removing it", 0) end
					if kind ~= types.SIGNAL then error("a script's build that removes a stop", 0) end
					local c = api.engine.getComponent(e, C.EDGE_OBJECT)
					local u = c and get(c, "param")
					if type(u) ~= "number" or u < 0 or u > 1 then error("a removed signal with no place on its edge", 0) end
					entry.remove[#entry.remove + 1] = {
						at = u, model = resName(get(c, "edgeObjectConstruction"), "the removed signal's construction"),
					}
					removing[e] = nil
				end
			end
			table.sort(entry.remove, function(a, b) return a.at < b.at end)
			if #entry.add > 0 or #entry.remove > 0 then edges[#edges + 1] = entry end
		end
		if next(removing) ~= nil then error("a signal build that removes an object of no edge it rebuilds", 0) end
		for k = 1, #objectAdds do
			if not used[k] then error("a new signal on no edge", 0) end
		end
		if #edges == 0 then return false end
		return { PlaceSignals = {
			model = model or "", one_way = oneWay == true, params = params or {}, edges = edges,
		} }
	end)
	if not ok then return nil, tostring(action) end
	return action
end

-- The action table of a street or track tool's proposal; false for a
-- proposal of nothing; or nil and why the room cannot carry it.
-- The town buildings a road or track clears go with it: every game's build
-- clears them again (tpf3mp/apply.lua, gatherBuildings); any other
-- construction in the way is refused.
function engine.captureBuild(proposal, network)
	local ok, capture = pcall(engine.fromProposal, proposal, network, "town")
	if not ok then return nil, tostring(capture) end
	if capture == nil then return false end
	return roads.capture(capture, engine.world())
end

-- A road or track modifier's build (the upgrade tools: tram tracks, bus
-- lanes, a street or track type, decorations, the towns' lock, the
-- company's ownership): the edges it rebuilds, each with its new template,
-- decorations, lock and owner, their stops kept in place, as a BuildRoad
-- or BuildTrack of the network of its first edge. The town buildings it
-- clears along the road every game's build clears again.
function engine.captureModify(proposal)
	local street = get(proposal, "proposal")
	if street and #list(get(street,"addedNodes")) == 0 and #list(get(street,"addedSegments")) == 0
		and #list(get(street,"removedNodes")) == 0 and #list(get(street,"removedSegments")) == 0 then
		return junctions.edit(proposal)
	end
	local ok, capture = pcall(engine.fromProposal, proposal, nil, "town")
	if not ok then return nil, tostring(capture) end
	if capture == nil then return false end
	return roads.capture(capture, engine.world())
end


-- An edge's lanes and decorations, as an action carries them (apply.lua
-- lays a track piece again with them).
engine.lanesOf, engine.decorationsOf = lanesOf, decorationsOf

return engine
