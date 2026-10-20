-- tpf3mp/toolchain.lua -- the street and track tools continue from the
-- segment the room built: the GUI's half (docs/HOOKS.md, "The street and
-- track tools continue").
--
-- The hook keeps the player's last click of the street or track tool and,
-- once the room built it, the edge that ends where the click did. Each GUI
-- frame this checks that edge in the GUI's own state: the same edge, with
-- the same nodes, its clicked end where the room put it. That also shows
-- the GUI's state already has the room's build. It counts the edges at
-- that end's node, which decide whether the tool keeps the segment's
-- direction. The hook then lets the tool continue from its own frame.
--
-- Pure Lua against the game's `api`; the tests give it a fake one.

local toolchain = {}

-- How far the GUI's node may lie from the room's end, in metres.
toolchain.TOLERANCE = 0.005

local function get(value, key)
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

local function length(list)
	if list == nil then return nil end
	local ok, n = pcall(function() return #list end)
	if ok and type(n) == "number" then return n end
	return nil
end

-- The edge the hook named, in the GUI's state: nil when it is not there as
-- the room built it, else the number of street edges and of other edges
-- at the clicked end's node.
function toolchain.check(api, edge, node0, node1, endIndex, x, y, z)
	local C = api.type.ComponentType
	local e = api.engine.getComponent(edge, C.BASE_EDGE)
	if e == nil or get(e, "node0") ~= node0 or get(e, "node1") ~= node1 then return nil end
	local node = endIndex == 1 and node1 or node0
	local n = api.engine.getComponent(node, C.BASE_NODE)
	local p = n and get(n, "position")
	if p == nil then return nil end
	local px, py, pz = get(p, "x"), get(p, "y"), get(p, "z")
	if type(px) ~= "number" or type(py) ~= "number" or type(pz) ~= "number" then return nil end
	local dx, dy, dz = px - x, py - y, pz - z
	if dx * dx + dy * dy + dz * dz > toolchain.TOLERANCE * toolchain.TOLERANCE then return nil end
	local segments = api.engine.system.streetSystem.getNodeSegments(node)
	local count = length(segments)
	if count == nil then return nil end
	local streets, others = 0, 0
	for i = 1, count do
		local s = get(segments, i)
		if api.engine.getComponent(s, C.BASE_EDGE_STREET) ~= nil then
			streets = streets + 1
		elseif api.engine.getComponent(s, C.BASE_EDGE) ~= nil then
			others = others + 1
		end
	end
	return streets, others
end

-- One GUI frame: asks the hook for the click's built edge, checks it and
-- tells the hook what the GUI's state shows. Never raises.
function toolchain.frame(link, api)
	local edge, node0, node1, endIndex, x, y, z = link:chainPoll()
	if edge == nil then return end
	local ok, streets, others = pcall(toolchain.check, api, edge, node0, node1, endIndex, x, y, z)
	if ok and streets ~= nil then link:chainSeen(edge, streets, others) end
end

return toolchain
