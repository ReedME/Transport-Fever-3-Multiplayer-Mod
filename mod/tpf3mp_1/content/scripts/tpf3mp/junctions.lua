-- Junction configuration capture/replay. Layout and field semantics follow
-- TF3 build 40408's shipped api/tealdef/api/{type,engine}.d.tl. Entity ids
-- stay inside this adapter; actions use positions and resource names.
-- PLAN.md Part 3: enable in matching acceptance-test copies on both games,
-- then change the default only after the two-player sandbox check passes.
-- The upstream double-slip exception (U-D30) is retained for acceptance
-- copies only. ReedME has not adopted that exception; ordinary rooms refuse it.
local junctions = { strict_junctions = false, double_slip_only = false }

local AWAITING = "junction tools await the two-player acceptance test (strict_junctions)"

local function get(v, k)
	if v == nil then return nil end
	local ok, x = pcall(function() return v[k] end)
	if ok then return x end
end
local function list(v)
	if v == nil then return {} end
	local out = {}
	for i = 1, #v do out[i] = v[i] end
	return out
end
local function component(id, kind, source)
	local api = source or api
	local c = api.type.ComponentType[kind]
	if c ~= nil then return api.engine.getComponent(id, c) end
end
local function pos(v)
	if v == nil then error("a junction with no position", 0) end
	local p = { x = get(v, "x") or get(v, 1), y = get(v, "y") or get(v, 2), z = get(v, "z") or get(v, 3) }
	for _, k in ipairs({ "x", "y", "z" }) do
		local n = p[k]
		if type(n) ~= "number" or n ~= n or math.abs(n) == math.huge then error("an invalid junction position", 0) end
	end
	return p
end
local function near(a, b)
	return math.abs(a.x-b.x) <= 0.002 and math.abs(a.y-b.y) <= 0.002 and math.abs(a.z-b.z) <= 0.002
end
local function pointKey(p)
	return string.format("%.0f,%.0f,%.0f", p.x*1000, p.y*1000, p.z*1000)
end
local function nodeKey(n) return n.network .. ":" .. pointKey(n.at) end
local function edgeKey(e)
	local a, b = pointKey(e.ends.a), pointKey(e.ends.b)
	if a > b then a, b = b, a end
	return e.network .. ":" .. a .. ">" .. b
end

-- A view including proposed geometry. Negative ids are used only while
-- reading a proposal and are immediately replaced by portable references.
local readComponent = component
local function world(street, source, snapshot)
	local api = source or api
	local function component(id, kind)
		if snapshot and kind == "BASE_EDGE" then
			local c = snapshot.edges[id]
			if c == nil then c = readComponent(id, kind, api) or false snapshot.edges[id] = c end
			return c or nil
		end
		return readComponent(id, kind, api)
	end
	local function segments(id, kind)
		local cache = snapshot and snapshot[kind]
		if cache and cache[id] then return cache[id] end
		local edges
		if snapshot and snapshot.maps then
			edges = list(snapshot.maps[kind][id])
		else
			edges = list(api.engine.system.streetSystem["getNode" .. kind .. "Segments"](id))
		end
		if cache then cache[id] = edges end
		return edges
	end
	local w = { nodes = {}, edges = {}, removed = {} }
	for _, n in ipairs(list(get(street, "addedNodes") or get(street, "nodesToAdd"))) do
		w.nodes[n.entity] = pos(n.comp.position)
	end
	for _, e in ipairs(list(get(street, "addedSegments") or get(street, "edgesToAdd"))) do w.edges[e.entity] = e end
	for _, e in ipairs(list(get(street, "removedSegments") or get(street, "edgesToRemove"))) do
		w.removed[type(e) == "number" and e or e.entity] = true
	end
	function w.position(id)
		return w.nodes[id] or pos(get(component(id, "BASE_NODE"), "position"))
	end
	function w.edge(id)
		local proposed = w.edges[id]
		local c = proposed and proposed.comp or component(id, "BASE_EDGE")
		if not c then error("a junction edge no longer exists", 0) end
		local node0, node1 = c.node0, c.node1
		local network
		if proposed then
			if proposed.type == 0 then network = "Street" elseif proposed.type == 1 then network = "Track" end
		else
			for _, kind in ipairs({ "Street", "Track" }) do
				for _, e in ipairs(segments(node0, kind)) do
					if e == id then network = kind end
				end
			end
		end
		if not network then error("a junction edge of unknown network", 0) end
		return { network = network, ends = { a = w.position(node0), b = w.position(node1) } }, c
	end
	function w.node(id)
		local network
		for _, kind in ipairs(id >= 0 and { "Street", "Track" } or {}) do
			local edges = segments(id, kind)
			if edges and #edges > 0 then network = kind break end
		end
		if not network then
			for _, e in pairs(w.edges) do
				if e.comp.node0 == id or e.comp.node1 == id then
					local kind = e.type == 0 and "Street" or "Track"
					if not network or kind == "Street" then network = kind end
				end
			end
		end
		if not network then error("a junction outside either network", 0) end
		return { network = network, at = w.position(id) }
	end
	return w
end

-- The live world (no proposal) with each node, edge and position read from
-- the engine once: for reading every junction at a checkpoint, where the
-- world cannot change between the reads and every node's connections name
-- the same few edges again and again.
local function remembered(source, baseEdges, maps)
	-- Lifetime is one rows() call. The network lane can lend the components
	-- it just read on this same simulation step; never retain across steps.
	local w = world(nil, source, { edges = baseEdges or {}, Street = {}, Track = {}, maps = maps })
	local position, edge, node = w.position, w.edge, w.node
	local positions, edges, nodes = {}, {}, {}
	function w.position(id)
		local p = positions[id]
		if p == nil then p = position(id) positions[id] = p end
		return p
	end
	function w.edge(id)
		local e = edges[id]
		if e == nil then e = { edge(id) } edges[id] = e end
		return e[1], e[2]
	end
	function w.node(id)
		local n = nodes[id]
		if n == nil then n = node(id) nodes[id] = n end
		return n
	end
	return w
end

local PREFERENCES = { Auto = "AUTO", Yes = "YES", No = "NO" }
-- `memo`, when given, keeps the preference and light names it looked up for
-- the configs after this one (junctions.rows reads every junction at once).
local function captureConfig(c, w, source, memo)
	local api = source or api
	local preference
	local value = c.trafficLightPreference
	if memo and memo.preferences then
		preference = memo.preferences[value]
	else
		local enums = api.type.enum.TrafficLightPreference
		for name, key in pairs(PREFERENCES) do if value == enums[key] then preference = name end end
		if memo and preference then
			memo.preferences = {}
			for name, key in pairs(PREFERENCES) do memo.preferences[enums[key]] = name end
		end
	end
	if not preference then error("unknown traffic light preference", 0) end
	local config = { connections = {}, crosswalks = {}, preference = preference, phases = {},
		double_slip = c.doubleSlipSwitch == true, custom_phases = c.userModifiedTrafficLightStates == true }
	for _, turn in ipairs(list(c.laneConnections)) do
		config.connections[#config.connections+1] = { incoming = w.edge(turn.segment0), lane_in = turn.lane0,
			outgoing = w.edge(turn.segment1), lane_out = turn.lane1, road = turn.withRoad == true, tram = turn.withTram == true }
	end
	for _, e in ipairs(list(c.crosswalks)) do config.crosswalks[#config.crosswalks+1] = w.edge(e) end
	local lights = c.trafficLightConfig
	if lights == nil then error("traffic light configuration did not read", 0) end
	local lightType = lights.trafficLightType
	if lightType ~= -1 then
		local names = memo and memo.lights
		config.light = names and names[lightType] or api.res.trafficLightTypeRep.getName(lightType)
		if type(config.light) ~= "string" or config.light == "" then error("unknown traffic light resource", 0) end
		if memo then
			memo.lights = names or {}
			memo.lights[lightType] = config.light
		end
	end
	for _, phase in ipairs(list(lights.states)) do
		config.phases[#config.phases+1] = { locked = list(phase.lockedLanes), duration = phase.duration,
			minimum = phase.minDuration, skip = phase.canSkip == true }
	end
	return config
end

-- Capture both updates and explicit resets; preserve order within a config
-- because a traffic-light phase's indices address that order.
function junctions.capture(street)
	local adds, removes = list(get(street, "nodeConfigsToAdd")), list(get(street, "nodeConfigsToRemove"))
	if #adds == 0 and #removes == 0 then return {} end
	local w, changes, seen = world(street), {}, {}
	for _, record in ipairs(adds) do
		if seen[record.entity] then error("a junction configured twice", 0) end
		seen[record.entity] = true
		changes[#changes+1] = { node = w.node(record.entity), config = captureConfig(record.comp, w) }
	end
	for _, id in ipairs(removes) do
		if not seen[id] then changes[#changes+1] = { node = w.node(id) } seen[id] = true end
	end
	table.sort(changes, function(a,b) return nodeKey(a.node) < nodeKey(b.node) end)
	return changes
end

-- A node configuration's settings in this game's own terms (edge entities,
-- the light's resource id, the preference's enum value), to compare two of
-- them at the same node.
local function plain(c)
	local lights = c.trafficLightConfig
	if lights == nil then error("traffic light configuration did not read", 0) end
	local out = { lanes = {}, walks = {}, phases = {}, preference = c.trafficLightPreference,
		light = lights.trafficLightType, double_slip = c.doubleSlipSwitch == true,
		custom_phases = c.userModifiedTrafficLightStates == true }
	for _, t in ipairs(list(c.laneConnections)) do
		out.lanes[#out.lanes+1] = t.segment0 .. ":" .. t.lane0 .. ">" .. t.segment1 .. ":" .. t.lane1 .. ":"
			.. tostring(t.withRoad == true) .. ":" .. tostring(t.withTram == true)
	end
	for _, e in ipairs(list(c.crosswalks)) do out.walks[#out.walks+1] = e end
	table.sort(out.walks)
	-- In whole milliseconds as the wire rounds them (half away from zero),
	-- so a phase compares alike before and after it crossed the wire.
	local function ms(seconds)
		local scaled = seconds * 1000
		local whole = math.floor(scaled)
		if scaled - whole >= 0.5 then whole = whole + 1 end
		return string.format("%d", whole)
	end
	for _, p in ipairs(list(lights.states)) do
		out.phases[#out.phases+1] = ms(p.duration) .. "/" .. ms(p.minDuration) .. "/"
			.. tostring(p.canSkip == true) .. ":" .. table.concat(list(p.lockedLanes), ",")
	end
	return out
end

-- Why `new` (nil for a reset) is not an edit of `old` (nil for a node with
-- no configuration) that a room carries with strict_junctions off, or nil
-- when it is. Before the full acceptance (PLAN.md Part 3) one edit goes, as
-- the junction window was seen to send it on build 40420: its double slip
-- switch turned on or off, and nothing else at the node changed, but for
-- the one side effect seen with it, the preference from Auto to No, and that
-- only at a node no street joins, where a light has nothing to stop. The
-- lane connections, crosswalks, light, phases and the custom-phase mark
-- stay exactly as they are. `enums` is TrafficLightPreference.
local function narrowWhyNot(old, new, streetless, enums)
	if new == nil then return "a reset of a junction's settings" end
	if old == nil then return "a junction with no settings yet" end
	if old.double_slip == new.double_slip then return "no double slip switch change" end
	local preference = old.preference == new.preference
		or (streetless and old.preference == enums.AUTO and new.preference == enums.NO)
	if not preference or old.light ~= new.light or old.custom_phases ~= new.custom_phases then
		return string.format("a change of its traffic lights (preference %s>%s, light %s>%s, custom phases %s>%s)",
			tostring(old.preference), tostring(new.preference), tostring(old.light), tostring(new.light),
			tostring(old.custom_phases), tostring(new.custom_phases))
	end
	if table.concat(old.walks, ",") ~= table.concat(new.walks, ",") then return "a change of its crosswalks" end
	if table.concat(old.phases, ";") ~= table.concat(new.phases, ";") then return "a change of its traffic light phases" end
	if table.concat(old.lanes, ";") ~= table.concat(new.lanes, ";") then return "a change of its lane connections" end
end

-- With strict_junctions off, raises unless putting `config` (a
-- BaseNodeConfig, or nil for a reset) at `node` is an edit narrowWhyNot
-- lets through; with it on, passes everything. Capture and replay both ask,
-- each against the configuration its own game has at the node.
function junctions.requireNarrow(node, config, source)
	if junctions.strict_junctions then return end
	if not junctions.double_slip_only then error(AWAITING, 0) end
	local api = source or api
	local streetless = false
	if node >= 0 then
		local system = api.engine.system.streetSystem
		streetless = #list(system.getNodeTrackSegments(node)) > 0 and #list(system.getNodeStreetSegments(node)) == 0
	end
	local old = node >= 0 and component(node, "BASE_NODE_CONFIG", api) or nil
	local before, after = old and plain(old), config and plain(config)
	local why = narrowWhyNot(before, after, streetless, api.type.enum.TrafficLightPreference)
	if why then
		local lanes = function(c) return c and tostring(#c.lanes) or "-" end
		error(AWAITING .. ": " .. why .. " (lanes " .. lanes(before) .. ">" .. lanes(after) .. ")", 0)
	end
end

function junctions.edit(proposal)
	local street = get(proposal, "proposal")
	if street == nil then error("a junction edit with no proposal", 0) end
	for _, name in ipairs({ "toAdd", "toRemove" }) do
		if #list(get(proposal, name)) > 0 then error("a junction edit with constructions", 0) end
	end
	for _, name in ipairs({ "addedNodes", "removedNodes", "addedSegments", "removedSegments", "nodesToAdd", "nodesToRemove", "edgesToAdd", "edgesToRemove", "edgeObjectsToAdd", "edgeObjectsToRemove" }) do
		if #list(get(street, name)) > 0 then error("a junction edit with geometry or edge objects", 0) end
	end
	local added = {}
	for _, record in ipairs(list(get(street, "nodeConfigsToAdd"))) do
		added[record.entity] = true
		junctions.requireNarrow(record.entity, record.comp)
	end
	for _, id in ipairs(list(get(street, "nodeConfigsToRemove"))) do
		if not added[id] then junctions.requireNarrow(id, nil) end
	end
	local changes = junctions.capture(street)
	if #changes == 0 then return false end
	return { EditJunctions = { changes = changes } }
end

local function makeConfig(config, edge, node)
	local c = api.type.BaseNodeConfig.new()
	local turns, walks = {}, {}
	local function at(e)
		local id, comp = edge(e)
		if comp.node0 ~= node and comp.node1 ~= node then error("a lane or crosswalk outside its junction", 0) end
		return id, comp
	end
	for _, turn in ipairs(config.connections) do
		local t = api.type.LaneConnection.new()
		local a, ac = at(turn.incoming)
		local b, bc = at(turn.outgoing)
		if turn.lane_in >= #ac.laneConfigs or turn.lane_out >= #bc.laneConfigs then error("the junction's lanes changed", 0) end
		t.segment0, t.lane0, t.segment1, t.lane1 = a, turn.lane_in, b, turn.lane_out
		t.withRoad, t.withTram = turn.road, turn.tram
		turns[#turns+1] = t
	end
	for _, e in ipairs(config.crosswalks) do walks[#walks+1] = at(e) end
	c.laneConnections, c.crosswalks = turns, walks
	c.trafficLightPreference = api.type.enum.TrafficLightPreference[PREFERENCES[config.preference]]
	if c.trafficLightPreference == nil then error("unknown traffic light preference", 0) end
	c.doubleSlipSwitch, c.userModifiedTrafficLightStates = config.double_slip, config.custom_phases
	local lights = c.trafficLightConfig
	local phases = {}
	for _, phase in ipairs(config.phases) do
		local p = api.type.TrafficLightState.new()
		local locked, seen = {}, {}
		for _, index in ipairs(phase.locked) do
			if index < 0 or index >= #turns+#walks or seen[index] then error("invalid locked lane in traffic lights", 0) end
			seen[index], locked[#locked+1] = true, index
		end
		if phase.minimum < 0 or phase.minimum > phase.duration or phase.duration > 86400 then error("invalid traffic light duration", 0) end
		p.lockedLanes, p.duration, p.minDuration, p.canSkip = locked, phase.duration, phase.minimum, phase.skip
		phases[#phases+1] = p
	end
	local light = config.light and api.res.trafficLightTypeRep.find(config.light) or -1
	if config.light and (type(light) ~= "number" or light < 0) then error("missing traffic light resource", 0) end
	lights.states, lights.trafficLightType = phases, light
	c.trafficLightConfig = lights
	return c
end

-- The changes that name none of `nodes` (node references) and none of
-- `edges` (edge references) as their junction, a turn's end or a
-- crosswalk; then those that do. A construction's own street, which the
-- construction makes again itself (tpf3mp/apply.lua ownStreets), is left
-- out of its build, and the tool's settings at its own nodes, or at a
-- junction its entrance joins, with it: they name a node or an edge the
-- build does not make, and the construction and its refresh give that
-- junction the game's own settings, the same in every game.
function junctions.without(changes, nodes, edges)
	local function sameEdge(x)
		for _, e in ipairs(edges or {}) do
			local a, b = x.ends, e.ends
			if x.network == e.network and ((near(a.a, b.a) and near(a.b, b.b)) or (near(a.a, b.b) and near(a.b, b.a))) then
				return true
			end
		end
		return false
	end
	local function names(change)
		for _, n in ipairs(nodes or {}) do
			if change.node.network == n.network and near(change.node.at, n.at) then return true end
		end
		local c = change.config
		if c == nil then return false end
		for _, turn in ipairs(c.connections or {}) do
			if sameEdge(turn.incoming) or sameEdge(turn.outgoing) then return true end
		end
		for _, e in ipairs(c.crosswalks or {}) do
			if sameEdge(e) then return true end
		end
		return false
	end
	local kept, left = {}, {}
	for _, change in ipairs(changes or {}) do
		if names(change) then left[#left + 1] = change else kept[#kept + 1] = change end
	end
	return kept, left
end

-- The configurations at `nodes` again, each reference to an edge in
-- `renames` (old entity -> { id = the new one, comp = its component })
-- naming the new edge: for edges a proposal rebuilds in place between the
-- same nodes (a stop placed or removed, signals placed along a track), all
-- in one, so a junction between two rebuilt edges names both. The other
-- edges keep their entities, so nothing is searched for by position. The
-- turns, crosswalks and light phases keep their order, and the mark that
-- the player changed the turns by hand stays; one that no longer fits its
-- edges' lanes, or a mark that cannot be kept, raises, and the build fails.
function junctions.renamedAll(proposal, nodes, renames)
	local adds, removes, seen = {}, {}, {}
	local function same(id) return id end
	for _, node in ipairs(nodes) do
		local c = not seen[node] and node >= 0 and component(node, "BASE_NODE_CONFIG")
		seen[node] = true
		if c then
			local config = captureConfig(c, { edge = same })
			local function edge(id)
				local r = renames[id]
				if r then return r.id, r.comp end
				local other = component(id, "BASE_EDGE")
				if not other then error("a junction edge no longer exists", 0) end
				return id, other
			end
			local n = api.type.BaseNodeLaneConnectionAndEntity.new()
			n.entity, n.comp = node, makeConfig(config, edge, node)
			local byHand
			pcall(function() byHand = c.userModifiedLaneConnections end)
			if byHand == true and not pcall(function() n.comp.userModifiedLaneConnections = true end) then
				error("a junction's turns set by hand cannot be kept", 0)
			end
			adds[#adds+1] = n
			removes[#removes+1] = node
		end
	end
	if #adds > 0 then
		proposal.streetProposal.nodeConfigsToAdd = adds
		proposal.streetProposal.nodeConfigsToRemove = removes
	end
end

-- As renamedAll, for one edge `old` rebuilt as `new` (whose component is
-- `comp`).
function junctions.renamed(proposal, nodes, old, new, comp)
	return junctions.renamedAll(proposal, nodes, { [old] = { id = new, comp = comp } })
end

-- Add configs to a SimpleProposal. Match in three dimensions and reject
-- ambiguous parallel edges/nodes instead of picking a game's lowest id.
-- `preserve` names the existing nodes whose incident edges are rebuilt.
-- `gone` names edges the proposal removes otherwise (an edit's old
-- construction's own, constructionsToRemove): no setting may name them, and
-- a preserved node at one keeps no settings; the construction and its
-- refresh give it the game's own, the same in every game. `narrow`, when
-- given, is asked about each change's node and the configuration made for
-- it (nil for a reset) before it goes in, and raises to refuse it
-- (junctions.requireNarrow). Returns the nodes left so.
function junctions.into(proposal, changes, preserve, mine, gone, narrow)
	if #(changes or {}) == 0 and #(preserve or {}) == 0 then return end
	local s = proposal.streetProposal
	local w = world(s)
	for id in pairs(gone or {}) do w.removed[id] = true end
	local removedNodes = {}
	for _, id in ipairs(list(s.nodesToRemove)) do removedNodes[id] = true end
	-- Every node of either network with its position, and the edges the
	-- network's map lists at it; an edge is read (w.edge) only where a
	-- reference names a place at one of its ends. Every edge is listed at
	-- both its nodes, and one that matches a reference has a node near the
	-- reference's first end, so the edges at those nodes are every edge the
	-- whole world could match (2026-10-16: reading every edge of the world
	-- took ~600 ms of every road build that rebuilt an edge in a late game,
	-- a street upgrade included; its ~10,000 nodes alone take 40-65 ms).
	-- A node's position is read plainly where it is three finite numbers,
	-- as w.position reads it; anything else goes through w.position, which
	-- refuses it as before (its guarded reads cost ~20 us a node).
	local positions = {}
	local BASE_NODE = api.type.ComponentType.BASE_NODE
	local function at(node)
		local p = positions[node]
		if p then return p end
		local read, x, y, z = pcall(function()
			local c = api.engine.getComponent(node, BASE_NODE)
			local v = c and c.position
			if v == nil then return nil end
			return v.x, v.y, v.z
		end)
		if read and type(x) == "number" and type(y) == "number" and type(z) == "number"
			and x == x and y == y and z == z and math.abs(x) ~= math.huge and math.abs(y) ~= math.huge
			and math.abs(z) ~= math.huge then
			p = { x = x, y = y, z = z }
		else
			p = w.position(node)
		end
		positions[node] = p
		return p
	end
	-- The nodes by the metre square their position is in, so a place's
	-- nodes are looked for in the squares within `near` of it only (a
	-- junction's every turn and crosswalk names two places): those 3 mm
	-- around it, a millimetre more than `near` takes, for rounding. `+ 0`
	-- makes -0 the square of 0.
	local cells = {}
	local function cell(x, y) return (math.floor(x) + 0) .. "," .. (math.floor(y) + 0) end
	local maps, allNodes = {}, {}
	for _, kind in ipairs({ "Street", "Track" }) do
		maps[kind] = api.engine.system.streetSystem["getNode2" .. kind .. "EdgeMap"]()
		for node, ids in pairs(maps[kind]) do
			local p = at(node)
			if not removedNodes[node] then
				allNodes[#allNodes+1] = { id = node, network = kind, at = p }
			end
			local key = cell(p.x, p.y)
			local bucket = cells[key]
			if bucket == nil then bucket = {} cells[key] = bucket end
			bucket[#bucket+1] = { at = p, ids = ids }
		end
	end
	for id, p in pairs(w.nodes) do
		local n = w.node(id)
		allNodes[#allNodes+1] = { id = id, network = n.network, at = p }
	end
	local proposedEdges, read = {}, {}
	for id, e in pairs(w.edges) do proposedEdges[id] = { ref = w.edge(id), comp = e.comp } end
	-- The edges at the nodes near `p` that stay, and the proposal's own.
	local function edgesAt(p)
		local out, keys = {}, {}
		for _, x in ipairs({ p.x - 0.003, p.x + 0.003 }) do
			for _, y in ipairs({ p.y - 0.003, p.y + 0.003 }) do
				local key = cell(x, y)
				if not keys[key] then keys[key] = true keys[#keys+1] = key end
			end
		end
		local nodes = {}
		for _, key in ipairs(keys) do
			for _, n in ipairs(cells[key] or {}) do nodes[#nodes+1] = n end
		end
		for _, n in ipairs(nodes) do
			if near(n.at, p) then
				for _, id in ipairs(list(n.ids)) do
					if not w.removed[id] and not out[id] then
						local e = read[id]
						if e == nil then
							e = { ref = w.edge(id), comp = component(id, "BASE_EDGE") }
							read[id] = e
						end
						out[id] = e
					end
				end
			end
		end
		for id, e in pairs(proposedEdges) do out[id] = e end
		return out
	end
	local function resolveNode(ref)
		local found
		for _, n in ipairs(allNodes) do
			if n.network == ref.network and near(n.at, ref.at) then
				if found and found ~= n.id then error("ambiguous junction position", 0) end
				found = n.id
			end
		end
		if not found then error("the junction no longer exists", 0) end
		return found
	end
	local function resolveEdge(ref, allowMissing)
		local found, comp
		for id, e in pairs(edgesAt(ref.ends.a)) do
			local a, b = e.ref.ends, ref.ends
			if ref.network == e.ref.network and ((near(a.a,b.a) and near(a.b,b.b)) or (near(a.a,b.b) and near(a.b,b.a))) then
				if found then error("ambiguous junction edge", 0) end
				found, comp = id, e.comp
			end
		end
		if not found then
			if allowMissing then return nil end
			error("the junction's road or track changed", 0)
		end
		if mine and found >= 0 then mine(found, "junction edge") end
		return found, comp
	end
	local adds, removes, handled = {}, {}, {}
	local function add(node, c)
		local n = api.type.BaseNodeLaneConnectionAndEntity.new()
		n.entity, n.comp = node, c
		adds[#adds+1] = n
	end
	local function remove(node)
		if node >= 0 and component(node,"BASE_NODE_CONFIG") ~= nil then removes[#removes+1] = node end
	end
	for _, change in ipairs(changes or {}) do
		local node = resolveNode(change.node)
		if handled[node] then error("a junction changed twice", 0) end
		handled[node] = true
		if mine and node >= 0 then
			for _, kind in ipairs({"Street", "Track"}) do
				for _, id in ipairs(list(api.engine.system.streetSystem["getNode"..kind.."Segments"](node))) do mine(id,"junction edge") end
			end
		end
		remove(node)
		local made = change.config and makeConfig(change.config, resolveEdge, node) or nil
		if narrow then narrow(node, made) end
		if made then add(node, made) end
	end
	-- A geometry rebuild must not throw away settings. The original turns
	-- keep their order (and hence phase indices); their old edges are mapped
	-- to the unique replacement leaving this same node in the same direction.
	local left = {}
	local function atGone(node)
		for _, kind in ipairs({"Street", "Track"}) do
			for _, id in ipairs(list(api.engine.system.streetSystem["getNode"..kind.."Segments"](node))) do
				if gone[id] then return true end
			end
		end
		return false
	end
	for _, node in ipairs(preserve or {}) do
		if gone and not handled[node] and not removedNodes[node] and node >= 0 and atGone(node) then
			handled[node] = true
			-- Its settings go as a change of them would: only where the
			-- acting company may change every edge at it (D21).
			if mine then
				for _, kind in ipairs({"Street", "Track"}) do
					for _, id in ipairs(list(api.engine.system.streetSystem["getNode"..kind.."Segments"](node))) do
						mine(id, "junction edge")
					end
				end
			end
			remove(node)
			left[#left+1] = node
		end
		if not handled[node] and not removedNodes[node] then
			handled[node] = true
			local old = component(node, "BASE_NODE_CONFIG")
			if old then
				local base = world(nil)
				local config = captureConfig(old, base)
				local function remap(ref)
					local id, comp = resolveEdge(ref, true)
					if id then return id, comp end
					-- A split of a curved road preserves the endpoint tangent,
					-- not the chord between its old endpoints. Find that exact
					-- original edge and require one replacement with its tangent.
					local original
					for gone in pairs(w.removed) do
						local r, c = base.edge(gone)
						if edgeKey(r) == edgeKey(ref) then
							if original then error("ambiguous original junction edge",0) end
							original = c
						end
					end
					if not original then error("missing original junction edge",0) end
					local function direction(c)
						local d = pos(c.node0 == node and c.tangent0 or c.tangent1)
						local sign = c.node0 == node and 1 or -1
						return d.x*sign,d.y*sign,d.z*sign
					end
					local dx,dy,dz = direction(original)
					local len = math.sqrt(dx*dx+dy*dy+dz*dz)
					local found, c
					for candidate,e in pairs(w.edges) do
						local n0,n1 = e.comp.node0,e.comp.node1
						if n0 == node or n1 == node then
							local er = proposedEdges[candidate].ref
							local x,y,z = direction(e.comp)
							local l = math.sqrt(x*x+y*y+z*z)
							if er.network == ref.network and len > 0 and l > 0 and (x*dx+y*dy+z*dz)/(l*len) > 0.99999 then
								if found then error("ambiguous replacement for a junction edge", 0) end
								found,c = candidate,e.comp
							end
						end
					end
					if not found then error("cannot preserve junction settings across this rebuild", 0) end
					return found,c
				end
				remove(node)
				add(node, makeConfig(config, remap, node))
			end
		end
	end
	if #adds > 0 then s.nodeConfigsToAdd = adds end
	if #removes > 0 then s.nodeConfigsToRemove = removes end
	return left
end

-- Canonical, portable rows for checkpoints. Phase indices are expressed as
-- the turns/crosswalks they lock, so local entity/vector ordering is irrelevant.
-- One node's row, or nil where it has no configuration; `w`, `memo` and `key`
-- are shared by one read.
local function junctionRow(api, w, memo, key, node)
	local c = component(node,"BASE_NODE_CONFIG",api)
	if not c then return nil end
	-- Where the hook decodes junctions, every row is of an owned copy:
	-- copying can lay the crosswalk set out anew, and a phase names its
	-- crosswalks by their order there (docs/HOOKS.md). The copy is made
	-- whoever decodes it; junctions.decoders = false reads its fields here.
	local native = tpf3mp_native
	local copy = api.type.BaseNodeConfig and api.type.BaseNodeConfig.new
	if native and type(native.junctionConfig) == "function" and type(copy) == "function" then
		local owned = copy(c)
		c = junctions.decoders ~= false and native.junctionConfig(owned) or owned
	end
	local v, lanes = captureConfig(c,w,api,memo), {}
	for _, t in ipairs(v.connections) do lanes[#lanes+1] = key(t.incoming)..":"..t.lane_in..">"..key(t.outgoing)..":"..t.lane_out..":"..tostring(t.road)..":"..tostring(t.tram) end
	for _, e in ipairs(v.crosswalks) do lanes[#lanes+1] = "walk:"..key(e) end
	local sorted = list(lanes) table.sort(sorted)
	local phases = {}
	for _, phase in ipairs(v.phases) do
		local locked = {}
		for _, i in ipairs(phase.locked) do
			if not lanes[i+1] then error("traffic phase references no lane",0) end
			locked[#locked+1] = lanes[i+1]
		end
		table.sort(locked)
		phases[#phases+1] = string.format("%.3f/%.3f/%s:%s",phase.duration,phase.minimum,tostring(phase.skip),table.concat(locked,","))
	end
	return nodeKey(w.node(node)).."|"..table.concat(sorted,";").."|"..v.preference.."|"..(v.light or "default").."|"..tostring(v.double_slip).."|"..tostring(v.custom_phases).."|"..table.concat(phases,";")
end

-- What one read of the junctions shares: the remembered world (lent the
-- edges and maps the caller read on this same step), the names looked up,
-- and each edge's key, made once (remembered() gives an edge the same table
-- every time, and every junction at its ends names it again).
local function reading(api, baseEdges, maps)
	local w, memo, keys = remembered(api, baseEdges, maps), {}, {}
	local function key(e)
		local k = keys[e]
		if k == nil then k = edgeKey(e) keys[e] = k end
		return k
	end
	return w, memo, key
end

function junctions.rows(api, baseEdges, selectedNodes)
	-- These complete maps already contain the adjacency needed below. Fetching
	-- each node's segments again crosses the engine boundary thousands of times.
	local maps
	if not selectedNodes then
		maps = { Street = api.engine.system.streetSystem.getNode2StreetEdgeMap(),
			Track = api.engine.system.streetSystem.getNode2TrackEdgeMap() }
	end
	local w, memo, key = reading(api, baseEdges, maps)
	local rows, seen = {}, {}
	for _, kind in ipairs(selectedNodes and {"Selected"} or {"Street", "Track"}) do
		for node in pairs(selectedNodes or maps[kind]) do
			if not seen[node] then
				seen[node] = true
				local row = junctionRow(api, w, memo, key, node)
				if row then rows[#rows+1] = row end
			end
		end
	end
	table.sort(rows)
	return rows
end

-- The rows of the junctions at `nodes` (node entities) only, as rows makes
-- them: those the hook's read leaves to this Lua (crates/tpf3mp-hook/src/
-- netread.rs, read_junctions). A node without a configuration is an error:
-- the hook read one there in this same update.
function junctions.rowsOf(api, nodes)
	local w, memo, key = reading(api)
	local rows = {}
	for i, node in ipairs(nodes) do
		local row = junctionRow(api, w, memo, key, node)
		if not row then error("a junction left to this Lua has no configuration", 0) end
		rows[i] = row
	end
	return rows
end

-- The names only the game's Lua gives a junction row: the traffic light
-- preferences by their value, and the light resources of `lights` (light
-- types the hook read) by their type, as captureConfig names them.
function junctions.names(api, lightTypes)
	local preferences, lights = {}, {}
	local enums = api.type.enum.TrafficLightPreference
	for name, key in pairs(PREFERENCES) do preferences[enums[key]] = name end
	for _, lightType in ipairs(lightTypes) do
		if lightType ~= -1 and lights[lightType] == nil then
			local light = api.res.trafficLightTypeRep.getName(lightType)
			if type(light) ~= "string" or light == "" then error("unknown traffic light resource", 0) end
			lights[lightType] = light
		end
	end
	return preferences, lights
end

-- The rows junctions.rows makes, from the hook's own read of the junctions
-- (crates/tpf3mp-hook/src/netread.rs): each row's head and tail as the hook
-- made them, and between them the two names only the game's Lua gives
-- (junctions.names). The hook's tests hold its rows to this.
function junctions.rowsFromParts(api, parts)
	local preferences, lights = junctions.names(api, parts.lights)
	local rows = {}
	for i, head in ipairs(parts.heads) do
		local preference = preferences[parts.preferences[i]]
		if not preference then error("unknown traffic light preference", 0) end
		local lightType = parts.lights[i]
		local light = lightType == -1 and "default" or lights[lightType]
		rows[i] = head .. "|" .. preference .. "|" .. light .. "|" .. tostring(parts.tails[i])
	end
	table.sort(rows)
	return rows
end

function junctions.summary(action)
	if type(action) ~= "table" then return nil end
	local changes
	if type(action.EditJunctions) == "table" then
		changes = action.EditJunctions.changes
	else
		local build = action.BuildRoad or action.BuildTrack
		changes = type(build) == "table" and type(build.polyline) == "table" and build.polyline.junctions or nil
	end
	if type(changes) ~= "table" or #changes == 0 then return nil end
	local parts = {}
	for _, change in ipairs(changes) do
		local n = change.node or {}
		local at = n.at or {}
		local where = string.format("%s(%.1f,%.1f)", tostring(n.network), tonumber(at.x) or 0, tonumber(at.y) or 0)
		local c = change.config
		if c then
			parts[#parts + 1] = string.format("%s{tl=%s light=%s lc=%d cw=%d phases=%d dss=%s custom=%s}", where,
				tostring(c.preference), tostring(c.light or "default"), #(c.connections or {}), #(c.crosswalks or {}),
				#(c.phases or {}), tostring(c.double_slip), tostring(c.custom_phases))
		else
			parts[#parts + 1] = where .. "{defaults}"
		end
	end
	return #changes .. " junction(s): " .. table.concat(parts, " ")
end

return junctions
