-- Level crossings' types in a room (docs/HOOKS.md, "Level crossing types").
--
-- A crossing's window changes its type natively; the hook reads the click
-- (crates/tpf3mp-hook/src/crossings.rs) as the crossing entity and its new
-- type index, and this names both portably: the crossing by the places of
-- all its nodes, sorted, and the type by its resource file
-- (tpf3mp_proto action::CrossingType). Every game then finds the one
-- crossing whose nodes are exactly there and gives it the type as the window
-- does (tpf3mp/apply.lua, through the hook's carrier), and reads the type
-- back.

local crossings = {}

-- What the player is told when the room does not carry a change.
crossings.REFUSED = "Not in multiplayer yet: changing a level crossing's type"
-- How far, in metres and in any direction, a node may be from its place.
crossings.TOLERANCE = 0.5

local function list(v)
	local out = {}
	if v == nil then return out end
	local ok, n = pcall(function() return #v end)
	if not ok or type(n) ~= "number" then return out end
	for i = 1, n do out[i] = v[i] end
	return out
end

local function rep()
	local r = api.res.railroadCrossingTypeRep
	if r == nil then error("the game has no railroad crossing types", 0) end
	return r
end

-- The RAILROAD_CROSSING component of `entity`, or nil.
function crossings.component(entity)
	if type(entity) ~= "number" or entity < 0 then return nil end
	local ok, c = pcall(api.engine.getComponent, entity, api.type.ComponentType.RAILROAD_CROSSING)
	if ok then return c end
	return nil
end

-- The nodes of crossing `c` (its component), as a Lua list.
function crossings.nodes(c)
	return list(c and c.nodes)
end

-- A place's key, in whole millimetres, as the room's wire rounds it.
local function mm(v) return math.floor(v * 1000 + 0.5) end
local function key(p) return { mm(p.x), mm(p.y), mm(p.z) } end
local function before(a, b)
	local ka, kb = key(a), key(b)
	for i = 1, 3 do
		if ka[i] ~= kb[i] then return ka[i] < kb[i] end
	end
	return false
end

-- The places of crossing `c`'s nodes ({ x =, y =, z = } in metres), sorted
-- as the wire wants them (by whole millimetres, x then y then z), and the
-- nodes in the same order. Raises when one does not read or two coincide.
function crossings.places(c)
	local nodes = list(c and c.nodes)
	if #nodes == 0 then error("a crossing with no node", 0) end
	local placed = {}
	for _, node in ipairs(nodes) do
		local ok, b = pcall(api.engine.getComponent, node, api.type.ComponentType.BASE_NODE)
		local p = ok and b and b.position
		if p == nil then error("a crossing node whose place does not read", 0) end
		placed[#placed + 1] = { node = node, at = { x = p.x or p[1], y = p.y or p[2], z = p.z or p[3] } }
	end
	table.sort(placed, function(a, b) return before(a.at, b.at) end)
	local places, ordered = {}, {}
	for i, n in ipairs(placed) do
		if i > 1 and not before(placed[i - 1].at, n.at) then error("two crossing nodes in one place", 0) end
		places[i], ordered[i] = n.at, n.node
	end
	return places, ordered
end

-- The resource file of crossing type `index`, when the game offers it here
-- (visible); raises otherwise.
function crossings.typeName(index)
	local r = rep()
	if type(index) ~= "number" or index < 0 then error("a crossing type that is not one", 0) end
	local ok, name = pcall(r.getName, index)
	if not ok or type(name) ~= "string" or name == "" then error("crossing type " .. tostring(index) .. " has no name", 0) end
	local okVisible, visible = pcall(r.isVisible, index)
	if okVisible and visible == false then error("crossing type " .. name .. " is not offered here", 0) end
	return name
end

-- The index of the crossing type named `name` in this game, when it offers
-- it (visible); raises otherwise.
function crossings.typeIndex(name)
	local r = rep()
	local ok, index = pcall(r.find, name)
	if not ok or type(index) ~= "number" or index < 0 then error("no crossing type " .. tostring(name), 0) end
	local okVisible, visible = pcall(r.isVisible, index)
	if okVisible and visible == false then error("crossing type " .. tostring(name) .. " is not offered here", 0) end
	return index
end

-- The window's change as the hook read it ({ crossingType = true, type =,
-- entity = } as a SetCrossingType action, or nil and why. Rebuilds carry
-- `nodes`, `removed`, or `junctions`; this action cannot preserve their edge
-- and junction changes, so they are refused even if the named crossing fits.
function crossings.capture(native)
	local ok, action = pcall(function()
		local entity, index = native and native.entity, native and native.type
		if native and (native.nodes ~= nil or native.removed ~= nil or native.junctions ~= nil) then
			error("a crossing rebuild also changes segments or junctions and is not carried", 0)
		end
		local c = crossings.component(entity)
		if c == nil then error("the crossing is gone", 0) end
		local name = crossings.typeName(index)
		if c.typeIndex == index then error("the crossing is of that type already", 0) end
		local places = crossings.places(c)
		return { SetCrossingType = { nodes = places, crossing_type = name } }
	end)
	if ok then return action end
	return nil, tostring(action)
end

local function near(a, b)
	local dx, dy, dz = a.x - b.x, a.y - b.y, a.z - b.z
	return dx * dx + dy * dy + dz * dz <= crossings.TOLERANCE * crossings.TOLERANCE
end

-- The crossings with a node within the tolerance of one of `places`: each
-- node on both a street and a track there, by the game's crossing system.
local function candidates(places)
	local streets = api.engine.system.streetSystem
	-- Held while they are read (tpf3mp/apply.lua, readNodes).
	local trackMap, streetMap = streets.getNode2TrackEdgeMap(), streets.getNode2StreetEdgeMap()
	local onTrack = {}
	for node in pairs(trackMap) do onTrack[node] = true end
	local found, seen = {}, {}
	for node in pairs(streetMap) do
		if onTrack[node] then
			local b = api.engine.getComponent(node, api.type.ComponentType.BASE_NODE)
			local p = b and b.position
			if p then
				local at = { x = p.x or p[1], y = p.y or p[2], z = p.z or p[3] }
				for _, place in ipairs(places) do
					if near(at, place) then
						local ok, crossing = pcall(api.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode, node)
						if ok and type(crossing) == "number" and crossing >= 0 and not seen[crossing] then
							seen[crossing] = true
							found[#found + 1] = crossing
						end
						break
					end
				end
			end
		end
	end
	return found
end

-- The one crossing whose nodes are exactly at `places` (sorted, as the
-- action names them): its entity and component. Raises when there is none,
-- or more than one.
function crossings.resolve(places)
	if type(places) ~= "table" or #places == 0 then error("a crossing named by no node", 0) end
	local match
	for _, entity in ipairs(candidates(places)) do
		local c = crossings.component(entity)
		local okPlaces, own = pcall(crossings.places, c)
		if c and okPlaces and #own == #places then
			-- Each named place near exactly one of the crossing's nodes, a
			-- different one each: whatever order a slight difference in
			-- place puts them in.
			local same, taken = true, {}
			for _, place in ipairs(places) do
				local hit
				for i, at in ipairs(own) do
					if near(at, place) then
						if hit then same = false end
						hit = i
					end
				end
				if hit == nil or taken[hit] then same = false end
				if hit then taken[hit] = true end
				if not same then break end
			end
			if same then
				if match then error("two crossings where the action names one", 0) end
				match = { entity = entity, comp = c }
			end
		end
	end
	if not match then error("the crossing is not there", 0) end
	return match.entity, match.comp
end

-- ---------------------------------------------------------- checkpoints
--
-- Every crossing's type, for the network lane (tpf3mp/lanes.lua), so two
-- games whose crossings differ split at the next checkpoint. A row names
-- the crossing by the places of its nodes in whole millimetres, sorted
-- (as the action does), and its type by its file: `crossing:x,y,z;x,y,z|
-- file`. No entity id: two games number them differently.

-- How the last listing of every crossing was made, for the log.
crossings.listed = nil

-- Whether `a` (an engine binding) answers here: a stand-in engine without
-- crossings has none of them, and then the world has no crossing rows.
local function hasCrossings(a)
	return type(a) == "table" and a.type and a.type.ComponentType
		and a.type.ComponentType.RAILROAD_CROSSING ~= nil
		and a.engine and a.engine.system and a.engine.system.railRoadCrossingSystem ~= nil
end

-- The crossing on `node`, or nil where there is none. Raises when the
-- engine does not answer: a check never takes a failed read for no crossing.
local function crossingOf(a, node)
	local ok, e = pcall(a.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode, node)
	if not ok then error("the crossing on node " .. tostring(node) .. " does not read", 0) end
	if type(e) == "number" and e >= 0 then return e end
	return nil
end

-- Every crossing of the world: listed by the engine where it lists them,
-- else from the nodes on both a street and a track. Raises when neither
-- reads.
local function everyCrossing(a)
	local kind = a.type.ComponentType.RAILROAD_CROSSING
	local ok, all = pcall(a.engine.getEntitiesWithComponent, kind)
	if ok and type(all) == "table" then
		crossings.listed = "listed " .. #all
		local out = {}
		for i = 1, #all do out[i] = all[i] end
		return out
	end
	local streets = a.engine.system.streetSystem
	local trackMap, streetMap = streets.getNode2TrackEdgeMap(), streets.getNode2StreetEdgeMap()
	local onTrack = {}
	for node in pairs(trackMap) do onTrack[node] = true end
	local out, seen = {}, {}
	for node in pairs(streetMap) do
		if onTrack[node] then
			local e = crossingOf(a, node)
			if e and not seen[e] then
				seen[e] = true
				out[#out + 1] = e
			end
		end
	end
	crossings.listed = "by nodes " .. #out
	return out
end

-- The checkpoint row of crossing `entity`, its node places (for a dump's
-- box), or raises when the crossing does not read.
function crossings.row(a, entity)
	local ok, c = pcall(a.engine.getComponent, entity, a.type.ComponentType.RAILROAD_CROSSING)
	if not ok or c == nil then error("level crossing " .. tostring(entity) .. " does not read", 0) end
	local places = crossings.places(c)
	local keys = {}
	for i, p in ipairs(places) do
		keys[i] = string.format("%.0f,%.0f,%.0f", mm(p.x), mm(p.y), mm(p.z))
	end
	-- By its file alone: an index differs between games (D8).
	local index = c.typeIndex
	local okName, name = pcall(a.res.railroadCrossingTypeRep.getName, index)
	if not okName or type(name) ~= "string" or name == "" then
		error("the type of level crossing " .. tostring(entity) .. " has no name", 0)
	end
	return "crossing:" .. table.concat(keys, ";") .. "|" .. name, places
end

-- The checkpoint rows of the crossings on `nodes` (a set of node entities),
-- or of every crossing when `nodes` is nil, sorted; and each one's node
-- places, by row. None where the engine has no crossings.
function crossings.rows(a, nodes)
	if not hasCrossings(a) then return {}, {} end
	local list
	if nodes == nil then
		list = everyCrossing(a)
	else
		list = {}
		local seen = {}
		for node in pairs(nodes) do
			local e = crossingOf(a, node)
			if e and not seen[e] then
				seen[e] = true
				list[#list + 1] = e
			end
		end
	end
	local rows, points = {}, {}
	for _, e in ipairs(list) do
		local row, places = crossings.row(a, e)
		rows[#rows + 1] = row
		points[row] = places
	end
	table.sort(rows)
	return rows, points
end

-- The names of the level crossing types `indices` (as the hook's part
-- read gives them, crates/tpf3mp-hook/src/netread.rs), `{ [index] = file }`,
-- for the hook's crossing rows. Raises for a type with no name: a row never
-- takes the game's own index (D8).
function crossings.names(a, indices)
	local out = {}
	for _, index in ipairs(indices or {}) do
		local ok, name = pcall(a.res.railroadCrossingTypeRep.getName, index)
		if not ok or type(name) ~= "string" or name == "" then
			error("level crossing type " .. tostring(index) .. " has no name", 0)
		end
		out[index] = name
	end
	return out
end

return crossings
