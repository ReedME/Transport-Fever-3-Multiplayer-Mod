-- tpf3mp/roads.lua -- a street or track tool's proposal as a BuildRoad or
-- BuildTrack action (tpf3mp_proto::action; docs/BUILDING.md, "The action
-- schema").
--
-- The action is the proposal as the tool made it, by positions instead of
-- entity ids: every node it adds, every edge it adds between them and the
-- existing nodes, and every existing edge and node it removes. Transport
-- Fever 3's tools state all of it (seen on build 40408: a street drawn onto
-- another's middle removes the old junction's node and the two edges at
-- it, and adds the junction, the new street and the old street rebuilt
-- through the junction), so nothing is re-derived from geometry: a receiver
-- repeats what the originator's tool decided. An edge that is not of the
-- build's own kind (a piece of the street it joins, the street a track
-- crosses) keeps its own network, template and style.
--
-- Learned in TpF2 Multiplayer (MIT, tpf2-multiplayer by silver2127,
-- github.com/silver2127/tpf2-multiplayer, 0.6.1.12), whose capture this
-- replaces: gate on edges, not on new nodes (a road between two existing
-- junctions adds no node); the pieces of a road the build crosses keep that
-- road's kind; a removal that cannot be placed ships nothing.
--
-- Pure Lua: the world is asked through the `world` argument, which
-- tpf3mp/engine.lua implements on the game's API and tests implement on a
-- table.
--
--   capture = {
--     network  = "Street" | "Track",      -- the tool's network
--     street, bus_lane, tram,             -- a road: its template, bus lane, "None" | "Plain" | "Electric"
--     track, catenary,                    -- a track: its template, catenary
--     style,                              -- the build's road style, or nil
--     nodes   = { { id = -1, pos = {x, y, z} }, ... },   -- the proposal's new nodes (ids < 0)
--     edges   = { { node0 =, node1 =, network =, tangent0 = {..}, tangent1 = {..},
--                   structure = "Ground" | { Bridge = file } | { Tunnel = file },
--                   template =, style =,
--                   decorations = { { name =, flag = } }, locked =, owned =,
--                   position0 =, position1 = }, ... },  -- where the edge itself ends, if known
--                                                         -- the proposal's new edges
--     removed = { { node0 =, node1 =, network = }, ... }, -- the existing edges it removes
--     removedNodes = { { id =, pos = {x, y, z} }, ... },   -- the existing nodes it removes
--     explicit = true | nil,               -- every link names its kind (a construction's streets)
--   }
--   world = {
--     nodePos(id)     -> {x, y, z} of an existing node, or nil
--     nodeNetwork(id) -> "Street" | "Track" of an existing node, or nil
--   }
--
-- Positions are metres, as the game gives them, and stay metres in the
-- action table: the hook turns them into the schema's millimetres
-- (tpf3mp_proto::lua), rounding and range checks included.

local roads = {}

-- A position or tangent (an array, or x/y/z fields as the game's Vec3f reads
-- in Lua) as {x =, y =, z =} in metres, or nil and why not.
local function vec3(v)
	if type(v) ~= "table" and type(v) ~= "userdata" then
		return nil, "not a vector: " .. tostring(v)
	end
	local out = {}
	for i, axis in ipairs({ "x", "y", "z" }) do
		local c = v[axis]
		if c == nil then c = v[i] end
		if type(c) ~= "number" or c ~= c then
			return nil, axis .. " is not a number: " .. tostring(c)
		end
		out[axis] = c
	end
	return out
end

-- The action for `capture`, or nil and why not. Never raises on bad input.
function roads.capture(capture, world)
	local ok, action, reason = pcall(roads.convert, capture, world)
	if not ok then return nil, tostring(action) end
	return action, reason
end

function roads.convert(capture, world)
	local own = capture.network
	if own ~= "Street" and own ~= "Track" then
		return nil, "unknown network " .. tostring(own)
	end
	local ownTemplate
	if own == "Street" then ownTemplate = capture.street else ownTemplate = capture.track end

	local newPos = {}
	for _, n in ipairs(capture.nodes or {}) do
		if type(n.id) ~= "number" or n.id >= 0 then
			return nil, "new node with a non-placeholder id " .. tostring(n.id)
		end
		newPos[n.id] = n.pos
	end
	local function posOf(id)
		if id < 0 then return newPos[id] end
		return world.nodePos(id)
	end

	local vertices, links, index = {}, {}, {}
	local function vertexFor(id)
		if index[id] then return index[id] end
		local p = posOf(id)
		if not p then return nil, "node " .. id .. " has no position" end
		local pos, err = vec3(p)
		if not pos then return nil, "node " .. id .. ": " .. err end
		local resolve
		if id >= 0 then
			local net = world.nodeNetwork(id)
			if not net then return nil, "node " .. id .. " is in no network" end
			resolve = { Node = net }
		else
			resolve = "New"
		end
		vertices[#vertices + 1] = { pos = pos, resolve = resolve }
		index[id] = #vertices - 1   -- the schema's indices start at 0
		return index[id]
	end

	for k, e in ipairs(capture.edges or {}) do
		if type(e.node0) ~= "number" or type(e.node1) ~= "number" then
			return nil, "edge " .. k .. " has no end nodes"
		end
		local i1, err1 = vertexFor(e.node0)
		if not i1 then return nil, err1 end
		local i2, err2 = vertexFor(e.node1)
		if not i2 then return nil, err2 end
		if i1 == i2 then return nil, "edge " .. k .. " joins a node to itself" end
		local t0, errT0 = vec3(e.tangent0)
		local t1, errT1 = vec3(e.tangent1)
		if not (t0 and t1) then return nil, "edge " .. k .. " tangent: " .. tostring(errT0 or errT1) end
		local link = { from = i1, to = i2, tangent0 = t0, tangent1 = t1, structure = e.structure or "Ground",
			decorations = e.decorations or {}, locked = e.locked == true, owned = e.owned == true,
			lanes = e.lanes or {}, precedence = e.precedence }
		-- Where the edge itself ends, where that is not its vertex: the track
		-- tool starts a track laid onto a street's end on the street's tram
		-- lane, beside the node, and the game joins the two there only
		-- (build 40420). Off the vertex by a millimetre or more, as the
		-- schema rounds.
		for _, at in ipairs({ { "position0", i1 }, { "position1", i2 } }) do
			local field, i = at[1], at[2]
			if e[field] ~= nil then
				local p, errP = vec3(e[field])
				if not p then return nil, "edge " .. k .. " " .. field .. ": " .. errP end
				local v = vertices[i + 1].pos
				if math.abs(p.x - v.x) >= 0.0005 or math.abs(p.y - v.y) >= 0.0005 or math.abs(p.z - v.z) >= 0.0005 then
					link[field] = p
				end
			end
		end
		-- The signals it takes from a track the build cuts (engine.keptInPlace).
		if e.kept and #e.kept > 0 then link.kept = e.kept end
		if capture.explicit or e.network ~= own or e.template ~= ownTemplate or e.style ~= capture.style then
			if e.network ~= "Street" and e.network ~= "Track" then
				return nil, "edge " .. k .. " is in no network"
			end
			if type(e.template) ~= "string" then return nil, "edge " .. k .. " has no road template" end
			link.kind = { network = e.network, template = e.template, style = e.style }
		end
		links[#links + 1] = link
	end
	-- Gate on edges, not on new nodes: a road joining two existing
	-- junctions adds no node and one edge.
	if #links == 0 then return nil, "no edges to build" end

	-- A removal that cannot be placed ships nothing: replayed without it, an
	-- upgrade doubles the edge.
	local removals = {}
	for k, r in ipairs(capture.removed or {}) do
		local a = type(r.node0) == "number" and posOf(r.node0)
		local b = type(r.node1) == "number" and posOf(r.node1)
		local pa = a and vec3(a)
		local pb = b and vec3(b)
		if not (pa and pb) then return nil, "removed edge " .. k .. " has no position here" end
		local network = r.network or own
		removals[#removals + 1] = { network = network, ends = { a = pa, b = pb } }
	end

	local removedNodes = {}
	for k, n in ipairs(capture.removedNodes or {}) do
		local p = n.pos or (type(n.id) == "number" and posOf(n.id))
		local at = p and vec3(p)
		local network = type(n.id) == "number" and world.nodeNetwork(n.id)
		if not (at and network) then return nil, "removed node " .. k .. " has no place here" end
		removedNodes[#removedNodes + 1] = { network = network, at = at }
	end

	local polyline = { vertices = vertices, links = links, removals = removals, removed_nodes = removedNodes,
		junctions = capture.junctions or {} }
	if own == "Street" then
		return { BuildRoad = {
			street = capture.street, style = capture.style, bus_lane = capture.bus_lane == true,
			tram = capture.tram or "None", polyline = polyline,
		} }
	end
	return { BuildTrack = {
		track = capture.track, style = capture.style, catenary = capture.catenary == true, polyline = polyline,
	} }
end

-- The transport modes by the bit a lane's `modes` has for each: the
-- TransportMode values 0 to 15 in the order build 40408's tealdef lists
-- them (api/type.d.tl). INFERRED that each value is its place there, as
-- tpf3mp/engine.lua reads a lane's modes keyed by value.
roads.MODES = { "PERSON", "CARGO", "CAR", "BUS", "TRUCK", "TRAM", "ELECTRIC_TRAM", "TRAIN",
	"ELECTRIC_TRAIN", "AIRCRAFT", "SHIP", "SMALL_AIRCRAFT", "SMALL_SHIP", "HELICOPTER", "TRAM_TRACK",
	"ELECTRIC_TRAM_TRACK" }

-- A road or track modifier's build, the upgrade tools' (tram tracks, bus
-- lanes, barriers, trees, a street or track type, catenary), in one line
-- for the log: a BuildRoad or BuildTrack whose every link rebuilds an edge
-- it removes, between the same places in the same direction. What each
-- edge has after it: its template, the transport modes of its lanes (a tram
-- track is TRAM_TRACK, catenary ELECTRIC_TRAIN or ELECTRIC_TRAM_TRACK), the
-- lanes' speeds, its decorations, the towns' lock and the company's
-- ownership. nil for any other build.
function roads.upgradeSummary(action)
	if type(action) ~= "table" then return nil end
	local body, network = action.BuildRoad, "street"
	if body == nil then body, network = action.BuildTrack, "track" end
	local polyline = type(body) == "table" and body.polyline
	if type(polyline) ~= "table" or type(polyline.links) ~= "table" or #polyline.links == 0 then return nil end
	local vertices, removals = polyline.vertices or {}, polyline.removals or {}
	local function same(p, q)
		return type(p) == "table" and type(q) == "table" and math.abs(p.x - q.x) < 0.05
			and math.abs(p.y - q.y) < 0.05 and math.abs(p.z - q.z) < 0.05
	end
	local taken = {}
	for _, link in ipairs(polyline.links) do
		local a, b = vertices[(link.from or -1) + 1], vertices[(link.to or -1) + 1]
		local found
		for k, r in ipairs(removals) do
			if not taken[k] and a and b and r.ends and same(a.pos, r.ends.a) and same(b.pos, r.ends.b) then
				found = k
				break
			end
		end
		if found == nil then return nil end
		taken[found] = true
	end
	local own = body.street or body.track
	local templates, decorations, modes = {}, {}, {}
	local locked, owned, lanes, fromTemplate = 0, 0, 0, 0
	local slow, fast
	local function add(set, value)
		if value ~= nil and not set[value] then
			set[value] = true
			set[#set + 1] = tostring(value)
		end
	end
	for _, link in ipairs(polyline.links) do
		add(templates, (link.kind and link.kind.template) or own)
		for _, d in ipairs(link.decorations or {}) do add(decorations, d.name) end
		if link.locked then locked = locked + 1 end
		if link.owned then owned = owned + 1 end
		if #(link.lanes or {}) == 0 then fromTemplate = fromTemplate + 1 end
		for _, lane in ipairs(link.lanes or {}) do
			lanes = lanes + 1
			local m = lane.modes or 0
			for bit = 0, #roads.MODES - 1 do
				if math.floor(m / 2 ^ bit) % 2 == 1 then modes[bit + 1] = true end
			end
			if type(lane.speed) == "number" then
				slow = math.min(slow or lane.speed, lane.speed)
				fast = math.max(fast or lane.speed, lane.speed)
			end
		end
	end
	local named = {}
	for bit, name in ipairs(roads.MODES) do
		if modes[bit] then named[#named + 1] = name end
	end
	local parts = { network .. " upgrade of " .. #polyline.links .. " edge(s) rebuilt in place",
		"template " .. table.concat(templates, ", ") }
	if lanes > 0 then
		parts[#parts + 1] = lanes .. " lane(s) carrying " .. (#named > 0 and table.concat(named, " ") or "nothing")
		if slow then parts[#parts + 1] = string.format("lane speeds %g to %g", slow, fast) end
	end
	if fromTemplate > 0 then parts[#parts + 1] = fromTemplate .. " edge(s) with their template's lanes" end
	parts[#parts + 1] = "decorations " .. (#decorations > 0 and table.concat(decorations, ", ") or "none")
	parts[#parts + 1] = "locked " .. locked .. ", owned " .. owned
	return table.concat(parts, "; ")
end

return roads
