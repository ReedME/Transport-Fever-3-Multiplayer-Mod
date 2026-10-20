-- tpf3mp/capture.lua -- a build the player made with the game's own tools,
-- as the action the room orders instead (docs/HOOKS.md, "The build tools").
--
-- The tools show every proposal they make to game scripts
-- (builder.proposalCreate), with the proposal as the game will build it;
-- the mod's game script keeps the action each makes, and hands the room the
-- one the player clicked. Everything is read from the proposal as it is, in
-- the game's units: metres, and plain fractions for the matrix.
--
-- A proposal the room cannot carry yet is not guessed at: the capture says
-- why, and the tool shows it.
--
-- Pure Lua; the tests hand it proposals of the game's shape.

local capture = {}

-- CalendarEditorDateSpeedControl sends a day length, with zero for a
-- paused date. Carry the exact integer rather than recomputing a factor.
function capture.calendarSpeed(_ctx, millisPerDay)
	if type(millisPerDay) ~= "number" or millisPerDay ~= math.floor(millisPerDay)
		or millisPerDay < 0 or millisPerDay > 2147483647 then
		error("calendar day length must be a non-negative signed integer", 0)
	end
	return { CalendarSpeed = { millis_per_day = millisPerDay } }
end

local function get(value, key)
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

local function length(list)
	if list == nil then return 0 end
	local ok, n = pcall(function() return #list end)
	if ok and type(n) == "number" then return n end
	return nil
end

local function sortedKeys(tbl)
	local keys = {}
	for key in pairs(tbl) do keys[#keys + 1] = key end
	table.sort(keys, function(a, b)
		if type(a) == type(b) then return a < b end
		return type(a) == "number"
	end)
	return keys
end

-- A construction's parameters as the schema's flat list (tpf3mp_proto
-- action::Param): nested tables become paths, "modules[3801].name"; a number
-- with no fraction is Int, any other Fixed; a boolean Bool, a string Text.
-- Returns the list, or nil and why.
function capture.params(tbl)
	local out = {}
	local function walk(node, path, depth)
		if depth > 8 then error("parameters nested deeper than 8") end
		for _, key in ipairs(sortedKeys(node)) do
			local value = node[key]
			local here
			if type(key) == "number" and key == math.floor(key) then
				here = path .. "[" .. string.format("%d", key) .. "]"
			elseif type(key) == "string" and key:match("^[%a_][%w_]*$") then
				here = path == "" and key or (path .. "." .. key)
			else
				error("a parameter named " .. tostring(key))
			end
			local kind = type(value)
			if kind == "table" then
				walk(value, here, depth + 1)
			elseif kind == "number" then
				if value == math.floor(value) then
					out[#out + 1] = { key = here, value = { Int = value } }
				else
					out[#out + 1] = { key = here, value = { Fixed = value } }
				end
			elseif kind == "boolean" then
				out[#out + 1] = { key = here, value = { Bool = value } }
			elseif kind == "string" then
				out[#out + 1] = { key = here, value = { Text = value } }
			else
				error("parameter " .. here .. " is a " .. kind)
			end
		end
	end
	local ok, why = pcall(walk, tbl, "", 0)
	if not ok then return nil, tostring(why) end
	return out
end

-- The schema's transform (action::Transform) of the game's 4x4 matrix: its
-- basis is elements 1-3, 5-7 and 9-11, its origin 13-15.
function capture.transform(m)
	local function at(i)
		local v = get(m, i)
		if type(v) ~= "number" then error("the matrix has no element " .. i) end
		return v
	end
	return {
		basis = { at(1), at(2), at(3), at(5), at(6), at(7), at(9), at(10), at(11) },
		origin = { x = at(13), y = at(14), z = at(15) },
	}
end

local function module(name)
	local loaded = package and package.loaded and package.loaded["tpf3mp." .. name]
	if loaded then return loaded end
	if ug_require then return ug_require("tpf3mp_1::/scripts/tpf3mp/" .. name .. ".lua") end
	return require("tpf3mp." .. name)
end

-- Which of an edit's removed street pieces are the old construction's own
-- (capture.ownStreets, below), and the street part without them.
local ownRemovals, withoutOwn

local function stockAirport(file)
	return file == "::/stations/air/airfield.con" or file == "::/stations/air/airport.con"
end

-- A stock airport's replacement proposal may remove its generated runway
-- signals as well as the construction. Allow only signal IDs that occur once
-- on removed segments frozen into the same old airport; every external object
-- change stays refused. Return the removed segments with those internal
-- signal references stripped, or nil and why.
local function airportRemovalSegments(street, file, oldConstruction, signalType, removes, removeCount)
	local oldFile = get(oldConstruction, "fileName")
	if not stockAirport(file) or oldFile ~= file then return nil, "not a replacement of the same stock airport" end
	local frozenEdges = {}
	local frozen = get(oldConstruction, "frozenEdges")
	local frozenCount = length(frozen)
	if frozenCount == nil or frozenCount == 0 then return nil, "the old airport has no frozen edges" end
	for i = 1, frozenCount do
		local id = get(frozen, i)
		if type(id) ~= "number" or id ~= math.floor(id) or id <= 0 or frozenEdges[id] then
			return nil, "the old airport's frozen edges are unreadable"
		end
		frozenEdges[id] = true
	end
	local removedIds = {}
	for i = 1, removeCount do
		local row = get(removes, i)
		local id = type(row) == "number" and row or get(row, "entity")
		if type(id) ~= "number" or id ~= math.floor(id) or id <= 0 or removedIds[id] then
			return nil, "the removed airport object IDs are unreadable or repeated"
		end
		removedIds[id] = true
	end
	local segments = get(street, "removedSegments")
	local count = length(segments)
	if count == nil then return nil, "the removed airport segments are unreadable" end
	local occurrences, safe, signature = {}, {}, {}
	for i = 1, count do
		local segment = get(segments, i)
		local edgeId, comp = get(segment, "entity"), get(segment, "comp")
		local objects = get(comp, "objects")
		local objectCount = length(objects)
		if type(edgeId) ~= "number" or edgeId ~= math.floor(edgeId) or edgeId < 0 or objectCount == nil then
			return nil, "a removed airport segment is unreadable"
		end
		local kept, filtered = {}, false
		for k = 1, objectCount do
			local pair = get(objects, k)
			local objectId, kind = get(pair, 1), get(pair, 2)
			if removedIds[objectId] then
				if kind ~= signalType or not frozenEdges[edgeId] then
					return nil, "a removed object is not a signal on an old airport edge"
				end
				occurrences[objectId] = (occurrences[objectId] or 0) + 1
				if occurrences[objectId] ~= 1 then return nil, "a removed airport signal has more than one carrier" end
				signature[#signature + 1] = table.concat({ objectId, edgeId, kind }, ":")
				filtered = true
			elseif objectCount > 0 then
				-- The airport's own edge may carry only the signals named for
				-- removal; an object on a road being split remains unsupported.
				return nil, "a removed edge carries an object outside the airport signal batch"
			end
		end
		if filtered then
			local compView = setmetatable({ objects = kept }, {
				__index = function(_, key) return get(comp, key) end,
			})
			safe[i] = setmetatable({ comp = compView }, {
				__index = function(_, key) return get(segment, key) end,
			})
		else
			safe[i] = segment
		end
	end
	for id in pairs(removedIds) do
		if occurrences[id] ~= 1 then return nil, "a removed airport signal has no unique old-edge carrier" end
	end
	table.sort(signature)
	return safe, table.concat(signature, "|")
end

-- Airport constructions generate runway/taxiway signals as part of their
-- own internal network. Construction replay regenerates that network, so
-- those signal records must not be carried as a separate road action. The
-- exception is limited to the stock airport signals: their proposal rows
-- all use resultEntity=-1, while added-edge object IDs are the unique reserved
-- range beginning at -400000000. Strip them only when the counts and signal
-- types agree and each carrier's entire component uses newly added nodes.
-- Other IDs, categories, and objects on external components stay refused.
--
-- Some builds expose the construction's frozen edge list on the proposal;
-- when it is present, it further narrows the allowed carrier edges. The
-- Steam 40408 airport proposal does not expose that list, so the all-new
-- isolated-component check is the conservative fallback.
local function constructionObjects(proposal, con, oldConstruction)
	local function refuse(reason) return nil, reason end
	local street = get(proposal, "proposal")
	local adds = get(street, "edgeObjectsToAdd")
	local removes = get(street, "edgeObjectsToRemove")
	local addCount, removeCount = length(adds), length(removes)
	if addCount == nil or removeCount == nil then
		return refuse("a proposal it cannot read", "counts unreadable")
	end
	local file = get(con, "fileName")
	local objectTypes = api.type.enum and api.type.enum.EdgeObjectType
	local signalType = objectTypes and objectTypes.SIGNAL
	if type(signalType) ~= "number" then
		if removeCount > 0 or addCount > 0 then return refuse("a build with a stop or signal", "signal enum", signalType) end
	end
	local keptRemovedSegments = get(street, "removedSegments")
	local removedSignature = ""
	if removeCount > 0 then
		local safe, why = airportRemovalSegments(street, file, oldConstruction, signalType, removes, removeCount)
		if safe == nil then return refuse("a build with a removed stop or signal", "object removals", why) end
		keptRemovedSegments = safe
		removedSignature = why
	end
	if addCount == 0 and removeCount == 0 then return proposal, "" end
	if not stockAirport(file) then
		return refuse("a build with a stop or signal", "construction file", file)
	end
	if addCount == 0 then
		local streetView = setmetatable({ removedSegments = keptRemovedSegments,
			edgeObjectsToAdd = {}, edgeObjectsToRemove = {} }, {
			__index = function(_, key) return get(street, key) end,
		})
		return setmetatable({ proposal = streetView }, { __index = function(_, key) return get(proposal, key) end }), ""
	end

	local addedNodes = get(street, "addedNodes")
	local nodeCount = length(addedNodes)
	local segments = get(street, "addedSegments")
	local segmentCount = length(segments)
	if nodeCount == nil or segmentCount == nil then
		return refuse("a proposal it cannot read", "graph counts", tostring(nodeCount) .. "/" .. tostring(segmentCount))
	end
	local newNodes = {}
	for i = 1, nodeCount do
		local id = get(get(addedNodes, i), "entity")
		if type(id) ~= "number" or id ~= math.floor(id) or id >= 0 or newNodes[id] then
			return refuse("a build with a stop or signal", "added node id", i .. ":" .. tostring(id))
		end
		newNodes[id] = true
	end

	local segmentIds, segmentNodes, adjacent = {}, {}, {}
	for i = 1, segmentCount do
		local seg = get(segments, i)
		local id, comp = get(seg, "entity"), get(seg, "comp")
		local a, b = get(comp, "node0"), get(comp, "node1")
		if type(id) ~= "number" or id ~= math.floor(id) or id >= 0 or segmentIds[id]
			or type(a) ~= "number" or a ~= math.floor(a) or type(b) ~= "number" or b ~= math.floor(b)
			or a == b then
			return refuse("a build with a stop or signal", "added segment shape",
				i .. ":" .. tostring(id) .. ":" .. tostring(a) .. ">" .. tostring(b))
		end
		segmentIds[id] = i
		segmentNodes[i] = { a, b }
		for _, node in ipairs({ a, b }) do
			adjacent[node] = adjacent[node] or {}
			adjacent[node][#adjacent[node] + 1] = i
		end
	end

	-- IDs of frozen edges, where the builder exposes them. If the list exists
	-- and is nonempty, an object carrier must be one of those edges as well.
	local frozenEdges = {}
	local construction = get(con, "construction")
	local frozenList = get(construction, "frozenEdges")
	local frozenCount = length(frozenList)
	if frozenCount == nil then return refuse("a proposal it cannot read", "frozen edges unreadable") end
	for i = 1, frozenCount do
		local id = get(frozenList, i)
		if type(id) ~= "number" or id ~= math.floor(id) then
			return refuse("a proposal it cannot read", "frozen edge id", i .. ":" .. tostring(id))
		end
		frozenEdges[id] = true
	end

	local function isolated(edgeIndex)
		local seenEdges, seenNodes, pending = {}, {}, { edgeIndex }
		while #pending > 0 do
			local current = table.remove(pending)
			if not seenEdges[current] then
				seenEdges[current] = true
				for _, node in ipairs(segmentNodes[current]) do
					if node >= 0 or not newNodes[node] then return false end
					if not seenNodes[node] then
						seenNodes[node] = true
						for _, neighbor in ipairs(adjacent[node]) do pending[#pending + 1] = neighbor end
					end
				end
			end
		end
		return true
	end

	local name, player = get(con, "name"), get(con, "playerEntity")
	if type(name) ~= "string" or name == "" or type(player) ~= "number" then
		return refuse("a build with a stop or signal", "construction owner/name",
			tostring(name) .. "/" .. tostring(player))
	end
	local rows, seenIds, repeated, allPlaceholder = {}, {}, false, true
	for i = 1, addCount do
		local object = get(adds, i)
		local id = get(object, "resultEntity")
		if type(id) ~= "number" or id ~= math.floor(id) or id >= 0
			or get(object, "category") ~= 2 or get(object, "name") ~= name
			or get(object, "playerEntity") ~= player then
			return refuse("a build with a stop or signal", "object row",
				i .. ":id=" .. tostring(id) .. ",category=" .. tostring(get(object, "category"))
					.. ",name=" .. tostring(get(object, "name")) .. ",player=" .. tostring(get(object, "playerEntity"))
					.. "; construction=" .. tostring(name) .. "/" .. tostring(player))
		end
		if seenIds[id] then repeated = true end
		if id ~= -1 then allPlaceholder = false end
		seenIds[id] = true
		rows[i] = object
	end
	if repeated and not allPlaceholder then
		return refuse("a build with a stop or signal", "duplicate result ids")
	end
	local reservedObjects = allPlaceholder
	local records, resultIds = {}, {}
	if not reservedObjects then
		for _, object in ipairs(rows) do
			local id = get(object, "resultEntity")
			records[id] = object
			resultIds[id] = true
		end
	end

	local occurrences, signature = {}, {}
	if removedSignature ~= "" then signature[#signature + 1] = "removed:" .. removedSignature end
	if reservedObjects then
		for _, object in ipairs(rows) do
			signature[#signature + 1] = table.concat({ tostring(get(object, "resultEntity")),
				tostring(get(object, "category")), tostring(get(object, "left")), tostring(name), tostring(player) }, ":")
		end
	end
	local keptSegments = {}
	for i = 1, segmentCount do
		local seg = get(segments, i)
		local comp = get(seg, "comp")
		local objects = get(comp, "objects")
		local objectCount = length(objects)
		if objectCount == nil then return refuse("a proposal it cannot read", "segment objects unreadable", i) end
		local keptObjects = {}
		local filtered = false
		for k = 1, objectCount do
			local pair = get(objects, k)
			local objectId, kind = get(pair, 1), get(pair, 2)
			if reservedObjects then
				local offset = type(objectId) == "number" and objectId == math.floor(objectId)
					and (-400000000 - objectId) or nil
				if offset == nil or offset < 0 or offset >= addCount then
					return refuse("a build with a stop or signal", "unreserved object",
						tostring(objectId) .. ":" .. tostring(kind) .. " on edge " .. tostring(get(seg, "entity")))
				end
				if kind ~= signalType then
					return refuse("a build with a stop or signal", "reserved object type",
						tostring(objectId) .. ":" .. tostring(kind))
				end
				occurrences[objectId] = (occurrences[objectId] or 0) + 1
				if occurrences[objectId] ~= 1 then
					return refuse("a build with a stop or signal", "reserved object duplicate", tostring(objectId))
				end
				local edgeId = get(seg, "entity")
				local isIsolated = isolated(i)
				if not isIsolated or (frozenCount > 0 and not frozenEdges[edgeId]) then
					return refuse("a build with a stop or signal", "reserved object carrier",
						tostring(objectId) .. ":edge=" .. tostring(edgeId) .. ",isolated=" .. tostring(isIsolated)
							.. ",frozen=" .. tostring(frozenCount == 0 or frozenEdges[edgeId]))
				end
				signature[#signature + 1] = table.concat({ objectId, edgeId,
					segmentNodes[i][1], segmentNodes[i][2], kind }, ":")
				filtered = true
			elseif records[objectId] then
				if kind ~= signalType then
					return refuse("a build with a stop or signal", "object type", tostring(objectId) .. ":" .. tostring(kind))
				end
				occurrences[objectId] = (occurrences[objectId] or 0) + 1
				if occurrences[objectId] ~= 1 then
					return refuse("a build with a stop or signal", "object duplicate", tostring(objectId))
				end
				local edgeId = get(seg, "entity")
				local object = records[objectId]
				local namedEdge = get(object, "edgeEntity")
				if (namedEdge ~= nil and namedEdge ~= edgeId) or not isolated(i)
					or (frozenCount > 0 and not frozenEdges[edgeId]) then
					return refuse("a build with a stop or signal", "object carrier",
						tostring(objectId) .. ":edge=" .. tostring(edgeId) .. ",named=" .. tostring(namedEdge)
							.. ",isolated=" .. tostring(isolated(i)) .. ",frozen=" .. tostring(frozenCount == 0 or frozenEdges[edgeId]))
				end
				signature[#signature + 1] = table.concat({ objectId, edgeId,
					segmentNodes[i][1], segmentNodes[i][2], kind }, ":")
				filtered = true
			else
				if type(objectId) == "number" and objectId < 0 then
					return refuse("a build with a stop or signal", "unmatched negative object",
						tostring(objectId) .. ":" .. tostring(kind) .. " on edge " .. tostring(get(seg, "entity")))
				end
				keptObjects[#keptObjects + 1] = pair
			end
		end
		if filtered then
			local compView = setmetatable({ objects = keptObjects }, {
				__index = function(_, key) return get(comp, key) end,
			})
			keptSegments[i] = setmetatable({ comp = compView }, {
				__index = function(_, key) return get(seg, key) end,
			})
		else
			keptSegments[i] = seg
		end
	end
	if reservedObjects then
		for offset = 0, addCount - 1 do
			local id = -400000000 - offset
			if occurrences[id] ~= 1 then
				return refuse("a build with a stop or signal", "reserved object without carrier",
					tostring(id) .. ":" .. tostring(occurrences[id] or 0))
			end
		end
	else
		for id in pairs(resultIds) do
			if occurrences[id] ~= 1 then
				return refuse("a build with a stop or signal", "object without carrier",
					tostring(id) .. ":" .. tostring(occurrences[id] or 0))
			end
		end
	end
	table.sort(signature)
	local streetView = setmetatable({
		addedSegments = keptSegments,
		removedSegments = keptRemovedSegments,
		edgeObjectsToAdd = {},
		edgeObjectsToRemove = {},
	}, { __index = function(_, key) return get(street, key) end })
	local proposalView = setmetatable({ proposal = streetView }, {
		__index = function(_, key) return get(proposal, key) end,
	})
	table.sort(signature)
	return proposalView, table.concat(signature, "|")
end

-- One construction placed with the construction tool: stations, depots and
-- the rest (tpf3mp_proto action::ConstructionBuild). Returns the action
-- table, or nil and why the room cannot carry it yet.
--
-- A proposal that replaces one construction of the player's with a new one
-- (an edit of its modules or parameters, an upgrade) is carried with
-- `replaces`, the old one by its file and place; every game removes it and
-- builds the new one in one proposal (tpf3mp/apply.lua).
--
-- The construction's own streets its script makes again wherever it is
-- built. The proposal's street part is what the tool built around it, and
-- travels with it (capture.connection): built without it, a station by a
-- road stood beside the road, its entrance a dead end, and no line could
-- reach it (seen on build 40408).
function capture.construction(proposal)
	local street = get(proposal, "proposal")
	for _, list in ipairs({ "addedNodes", "addedSegments", "removedNodes", "removedSegments",
		"edgeObjectsToAdd" }) do
		if length(street and get(street, list)) == nil then return nil, "a proposal it cannot read" end
	end
	-- Constructions in the way: town buildings the placement clears, which
	-- the replay clears again (gatherBuildings), and at most one other
	-- construction the new one replaces: a module edit or an upgrade,
	-- named by its file and where it stands (capture.replaced).
	local toRemove = get(proposal, "toRemove")
	local removed = length(toRemove)
	if removed == nil then return nil, "a proposal it cannot read" end
	local replaced, replaces
	for i = 1, removed do
		local entity = get(toRemove, i)
		local c = api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION)
		if (length(c and get(c, "townBuildings")) or 0) == 0 then
			if replaced ~= nil then return nil, "a construction that replaces more than one" end
			local why
			replaces, why = capture.replaced(c)
			if not replaces then return nil, why end
			replaced = { entity = entity, component = c }
		end
	end
	local toAdd = get(proposal, "toAdd")
	if length(toAdd) ~= 1 then return nil, "more than one construction at once" end
	local con = get(toAdd, 1)
	local file = get(con, "fileName")
	if type(file) ~= "string" or file == "" then return nil, "a construction of no file" end
	local name = get(con, "name")
	if replaced and (type(name) ~= "string" or name == "") then
		-- An edit keeps the construction's name, as the game's own
		-- upgrade does (mission_framework_util_entity.tl, upgradeConstruction).
		local ok, old = pcall(function() return api.engine.util.getEntityName(replaced.entity) end)
		if ok then name = old end
	end
	if type(name) ~= "string" or name == "" then return nil, "an unnamed construction" end
	local params = get(con, "params")
	if type(params) ~= "table" then
		local construction = get(con, "construction")
		params = construction and get(construction, "params")
	end
	if type(params) ~= "table" then return nil, "a construction without its parameters" end
	local list, why = capture.params(params)
	if not list then return nil, why end
	local ok, transform = pcall(capture.transform, get(con, "transf"))
	if not ok then return nil, tostring(transform) end
	local safeProposal, objectSignature = constructionObjects(proposal, con, replaced and replaced.component)
	if safeProposal == nil then return nil, objectSignature end
	if replaced then
		-- The street part of an edit is mostly the construction's own: every
		-- game makes the new one's again as it builds it, and removes the old
		-- one's with the old construction. What it changes around it (a new
		-- exit onto a road the station did not join, which splits that road
		-- through a new junction, 2026-10-03) travels as its connection, as a
		-- new construction's does, without what the old one takes with it.
		local action = { BuildConstruction = { file = file, transform = transform, params = list, name = name,
			replaces = replaces } }
		local ownSegments, ownNodes, others = ownRemovals(street, replaced.component)
		if not others then return action end
		local safeStreet = get(safeProposal, "proposal")
		local connection, whyNot = capture.connection(withoutOwn(safeStreet, ownSegments, ownNodes))
		local around = "a construction edit that changes the streets around it"
		if connection == nil then return nil, around .. ": " .. tostring(whyNot) end
		if connection == false then return nil, around end
		action.BuildConstruction.connection = connection
		return action
	end
	local connection, whyNot = capture.connection(safeProposal)
	if connection == nil then return nil, whyNot end
	return { BuildConstruction = { file = file, transform = transform, params = list, name = name,
		connection = connection or nil } }
end

-- The construction an edit replaces, as actions name one (tpf3mp_proto
-- action::ConstructionRef): its file and where it stands. Every game finds
-- it there (tpf3mp/apply.lua, constructionAt), and finds the new one there
-- again for the next edit: an edit keeps the file and the place, and entity
-- ids are no name (docs/BUILDING.md, "Module edits and upgrades"). Returns
-- the reference, or nil and why the room cannot name it.
function capture.replaced(component)
	if component == nil then return nil, "removing something that is no construction" end
	local file = get(component, "fileName")
	local t = get(component, "transf")
	local x, y, z = get(t, 13), get(t, 14), get(t, 15)
	if type(file) ~= "string" or file == "" or type(x) ~= "number" or type(y) ~= "number"
		or type(z) ~= "number" then
		return nil, "a construction the room cannot name"
	end
	return { file = file, at = { x = x, y = y, z = z } }
end

-- Which nodes and edges an edit's street part removes of the old
-- construction's own, as two sets of entities, and whether it removes
-- anything else. Track ends may not be in frozenNodes (Steam 40408: a
-- two-track station has 50 nodes, only 46 frozen). Such a node belongs to
-- the rebuild only if every incident edge is frozen in this construction
-- and is being removed; a shared endpoint touching external track is not
-- its own.
function ownRemovals(street, component)
	local own = { frozenNodes = {}, frozenEdges = {} }
	for _, key in ipairs({ "frozenNodes", "frozenEdges" }) do
		local list = get(component, key)
		for i = 1, (length(list) or 0) do own[key][get(list, i)] = true end
	end
	local removed, nodesGone, others = {}, {}, false
	local segments = get(street, "removedSegments")
	for i = 1, (length(segments) or 0) do
		local id = get(get(segments, i), "entity")
		if own.frozenEdges[id] then removed[id] = true else others = true end
	end
	local function ownEnd(id)
		local ok, edges = pcall(function() return api.engine.system.streetSystem.getNodeSegments(id) end)
		local n = ok and length(edges)
		if not n or n < 1 then return false end
		for i = 1, n do
			if not removed[get(edges, i)] then return false end
		end
		return true
	end
	local nodes = get(street, "removedNodes")
	for i = 1, (length(nodes) or 0) do
		local id = get(get(nodes, i), "entity")
		if own.frozenNodes[id] or ownEnd(id) then nodesGone[id] = true else others = true end
	end
	return removed, nodesGone, others
end

-- Whether an edit's street part removes only the old construction's own
-- nodes and edges: true, or nil and why not.
function capture.ownStreets(street, component)
	local _, _, others = ownRemovals(street, component)
	if others then return nil, "a construction edit that changes the streets around it" end
	return true
end

-- An edit's street part without the old construction's own removals
-- (ownRemovals), which every game removes with the old construction: in the
-- street part as well, the game would be asked to remove them twice.
function withoutOwn(street, ownSegments, ownNodes)
	local view = {}
	for _, key in ipairs({ "addedNodes", "addedSegments", "edgeObjectsToAdd", "edgeObjectsToRemove",
		"nodeConfigsToAdd", "nodeConfigsToRemove" }) do
		view[key] = get(street, key)
	end
	local function kept(key, own)
		local out, items = {}, get(street, key)
		for i = 1, (length(items) or 0) do
			local item = get(items, i)
			if not own[get(item, "entity")] then out[#out + 1] = item end
		end
		return out
	end
	view.removedSegments = kept("removedSegments", ownSegments)
	view.removedNodes = kept("removedNodes", ownNodes)
	return { proposal = view }
end

-- The module editor's edit, as the hook read it (tpf3mp_native.built,
-- crates/tpf3mp-hook/src/modules.rs): the construction it replaces, the new
-- one's file, parameters, matrix and name, and of its street part the
-- entities it removes, but only how many nodes and edges it adds. An edit
-- whose street part is the old construction's own needs no more
-- (capture.construction). One that changes the streets around it (a new
-- exit onto a road the station did not join, which splits that road,
-- 2026-10-03) is asked of the game again, as the construction menu asks it
-- for a construction's new parameters (api.engine.util.proposal
-- .createProposalReplaceConstruction, gui/construction/construction.tl):
-- with the editor's parameters it proposes the editor's street part (seen
-- on build 40408). Its street part travels only when it is the same edit
-- as far as the hook read it: the same construction replaced by the same
-- file, as many nodes and edges added, the same nodes and edges removed,
-- no stop or signal on either side, and the construction where the editor
-- put it; else the edit is refused, with why. The construction itself is
-- the one the hook read. Returns the action, or nil and why.
function capture.moduleEdit(native)
	local street = get(native, "proposal")
	local toRemove = get(native, "toRemove")
	local old = length(toRemove) == 1 and get(toRemove, 1) or nil
	local c = old and api.engine.getComponent(old, api.type.ComponentType.CONSTRUCTION)
	if c == nil or (length(get(c, "townBuildings")) or 0) > 0 then return capture.construction(native) end
	local _, _, others = ownRemovals(street, c)
	local con = get(get(native, "toAdd"), 1)
	local airportSignalEdit = stockAirport(get(con, "fileName"))
		and get(con, "fileName") == get(c, "fileName")
		and ((length(get(street, "edgeObjectsToAdd")) or 0) > 0
			or (length(get(street, "edgeObjectsToRemove")) or 0) > 0)
	if not others and not airportSignalEdit then return capture.construction(native) end
	local proposals = api.engine.util and api.engine.util.proposal
	local make = proposals and proposals.createProposalReplaceConstruction
	if make == nil then return nil, "a construction edit that changes the streets around it" end
	local ok, full = pcall(make, old, get(con, "params"))
	if not ok or full == nil then
		return nil, "a construction edit the game will not propose again: " .. tostring(full)
	end
	local function differs(what) return nil, "a construction edit the game proposes otherwise: " .. what end
	local fullRemove, fullAdd = get(full, "toRemove"), get(full, "toAdd")
	if length(fullRemove) ~= 1 or get(fullRemove, 1) ~= old then return differs("the construction it replaces") end
	if length(fullAdd) ~= 1 or get(get(fullAdd, 1), "fileName") ~= get(con, "fileName") then
		return differs("the construction it builds")
	end
	local okA, a = pcall(capture.transform, get(get(fullAdd, 1), "transf"))
	local okB, b = pcall(capture.transform, get(con, "transf"))
	if not okA or not okB then return differs("where it stands") end
	local function far(x, y) return math.abs(x - y) > 0.01 end
	for i = 1, 9 do if far(a.basis[i], b.basis[i]) then return differs("where it stands") end end
	for _, k in ipairs({ "x", "y", "z" }) do if far(a.origin[k], b.origin[k]) then return differs("where it stands") end end
	local fullStreet = get(full, "proposal")
	for _, key in ipairs({ "addedNodes", "addedSegments" }) do
		local n = length(get(street, key))
		if n == nil or length(get(fullStreet, key)) ~= n then return differs("what it adds") end
	end
	local fullCon = get(fullAdd, 1)
	local fullSafe = constructionObjects(full, fullCon, c)
	if fullSafe == nil then return nil, "a construction edit with a stop or signal" end
	-- This hook record contains counts but empty object rows and segment
	-- placeholders, so the regenerated proposal supplies the carrier mapping.
	-- Match its object counts and require every native object row to be empty.
	local nativeAdds, nativeRemoves = get(street, "edgeObjectsToAdd"), get(street, "edgeObjectsToRemove")
	local fullAdds, fullRemoves = get(fullStreet, "edgeObjectsToAdd"), get(fullStreet, "edgeObjectsToRemove")
	if length(nativeAdds) ~= length(fullAdds) or length(nativeRemoves) ~= length(fullRemoves) then
		return differs("its edge objects")
	end
	local function placeholders(items)
		for i = 1, length(items) or 0 do
			local row = get(items, i)
			if type(row) ~= "table" then return false end
			for _, key in ipairs({ "entity", "resultEntity", "category", "left", "playerEntity", "edgeEntity", "param", "model", "name" }) do
				if get(row, key) ~= nil then return false end
			end
		end
		return true
	end
	if not placeholders(nativeAdds) or not placeholders(nativeRemoves) then
		return differs("its edge objects")
	end
	local function entities(list)
		local out, seen = {}, {}
		for i = 1, (length(list) or 0) do
			local id = get(get(list, i), "entity")
			if type(id) ~= "number" or seen[id] then return nil end
			seen[id] = true
			out[#out + 1] = id
		end
		table.sort(out)
		return table.concat(out, ",")
	end
	for _, key in ipairs({ "removedNodes", "removedSegments" }) do
		local mine, theirs = entities(get(street, key)), entities(get(fullStreet, key))
		if mine == nil or theirs == nil or mine ~= theirs then return differs("what it removes") end
	end
	return capture.construction(full)
end

-- Keeps of a construction's network part only the edges joined, through each
-- other, to a node that exists or to what the build removes, and the new
-- nodes they use. The construction's own tracks and streets come in its
-- proposal too, as new edges between new nodes that reach nothing existing
-- (build 40408: a rail station on open ground proposes its platform track,
-- 24 edges through 25 new nodes); the construction builds those itself, and
-- built beside it they block it ("Construction Not Possible").
--
-- The junction settings the tool proposed on those own tracks go with them
-- (2026-10-07: a rail station snapped to a track end was refused in every
-- game, "the junction no longer exists"): their nodes are in no game's
-- build, and the construction gives its own junctions the game's own
-- settings, alike in every game. So a setting at a node left out, or whose
-- turns name an edge left out, is left out too (junctions.without).
local function joinedOnly(part)
	local parent = {}
	local function find(x)
		while parent[x] ~= x do x = parent[x] end
		return x
	end
	for _, e in ipairs(part.edges) do
		for _, n in ipairs({ e.node0, e.node1 }) do
			if parent[n] == nil then parent[n] = n end
		end
		local a, b = find(e.node0), find(e.node1)
		if a ~= b then parent[a] = b end
	end
	local joined = {}
	for n in pairs(parent) do
		if type(n) == "number" and n >= 0 then joined[find(n)] = true end
	end
	for _, r in ipairs(part.removed) do
		for _, n in ipairs({ r.node0, r.node1 }) do
			if parent[n] ~= nil then joined[find(n)] = true end
		end
	end
	local edges, used = {}, {}
	for _, e in ipairs(part.edges) do
		if joined[find(e.node0)] then
			edges[#edges + 1] = e
			used[e.node0], used[e.node1] = true, true
		end
	end
	local nodes, at = {}, {}
	for _, n in ipairs(part.nodes) do
		if used[n.id] then nodes[#nodes + 1] = n end
		at[n.id] = { x = n.pos[1], y = n.pos[2], z = n.pos[3] }
	end
	local ownNodes, ownEdges = {}, {}
	for _, e in ipairs(part.edges) do
		if not joined[find(e.node0)] and at[e.node0] and at[e.node1] then
			ownEdges[#ownEdges + 1] = { network = e.network, ends = { a = at[e.node0], b = at[e.node1] } }
			for _, n in ipairs({ e.node0, e.node1 }) do
				ownNodes[#ownNodes + 1] = { network = e.network, at = at[n] }
			end
		end
	end
	if #ownEdges > 0 and #(part.junctions or {}) > 0 then
		part.junctions = module("junctions").without(part.junctions, ownNodes, ownEdges)
	end
	part.edges, part.nodes = edges, nodes
end

-- The street and track changes a construction tool's proposal makes with its
-- construction (seen on build 40408: a bus station placed by a road rebuilds
-- the road through a new junction and adds an edge from the junction to the
-- station's own street node), as a polyline whose every link names its
-- kind; false when it makes none; nil and why the room cannot carry them.
function capture.connection(proposal, con)
	if con ~= nil then
		local safe, why = constructionObjects(proposal, con)
		if safe == nil then return nil, why end
		proposal = safe
	end
	local engine = module("engine")
	local ok, part = pcall(engine.fromProposal, proposal, nil, true)
	if not ok then return nil, tostring(part) end
	if part == nil then return false end
	local removes = #part.removed > 0 or #part.removedNodes > 0
	joinedOnly(part)
	if #part.edges == 0 then
		if removes then return nil, "a construction that removes streets and builds none" end
		return false
	end
	part.explicit = true
	local first = part.edges[1]
	part.network = first.network
	if part.network == "Street" then part.street = first.template else part.track = first.template end
	part.style = first.style
	local action, why = module("roads").capture(part, engine.world())
	if not action then return nil, why end
	local build = action.BuildRoad or action.BuildTrack
	-- The construction creates its own switches; those settings are not
	-- separate network edits and may exceed the action's junction limit.
	local kept = module("apply").ownJunctions(build.polyline)
	build.polyline.junctions = kept
	return build.polyline
end

-- A street or track tool's build (tpf3mp_proto action::RoadBuild,
-- TrackBuild), read off its proposal by tpf3mp/engine.lua and made an action
-- by tpf3mp/roads.lua. Returns the action table; false for a proposal of
-- nothing (the tool before its first point); or nil and why.
function capture.street(proposal)
	return module("engine").captureBuild(proposal, "Street")
end

function capture.track(proposal)
	return module("engine").captureBuild(proposal, "Track")
end

-- The road and track modifiers' builds (tpf3mp/engine.lua captureModify).
function capture.modify(proposal)
	return module("engine").captureModify(proposal)
end

function capture.junction(proposal)
	return module("junctions").edit(proposal)
end

-- An upgrade tool's build in one line for the log, or nil for any other
-- (tpf3mp/roads.lua upgradeSummary).
function capture.upgradeSummary(action)
	local ok, text = pcall(module("roads").upgradeSummary, action)
	if ok then return text end
	return nil
end


-- Most cells one Terraform action carries (tpf3mp_proto
-- action::MAX_TERRAIN_CELLS is 8192; half keeps every action well inside
-- the room's 48 KiB payload, two varints a cell).
capture.TERRAIN_CELLS = 4096

-- The side of the map's height cells, in metres
-- (api.engine.terrain.getBaseResolution, 4 m on build 40408), or nil and why.
function capture.terrainCell()
	local ok, resolution = pcall(function() return api.engine.terrain.getBaseResolution() end)
	local cell = ok and resolution and (resolution.x or resolution[1])
	if type(cell) ~= "number" or cell <= 0 or cell ~= cell then
		return nil, "the terrain's resolution does not read"
	end
	return cell
end

-- A terrain tool's stroke, as the hook read it at the click
-- (tpf3mp_native.built: `{ terrain = { x0 =, y0 =, width =, height =,
-- cells = { v1, w1, v2, w2, ... } } }`, crates/tpf3mp-hook/src/terrain.rs),
-- as Terraform actions (tpf3mp_proto action::Terraform): the grid's first
-- cell by its index times the cell size, its columns, and every cell's two
-- values, row by row, in bands of whole rows of at most TERRAIN_CELLS
-- cells, each its own action. Returns the list of actions, or nil and why.
function capture.terraform(built)
	local t = type(built) == "table" and built.terrain
	if type(t) ~= "table" then return nil, "no terrain grid" end
	local function whole(v) return type(v) == "number" and v == math.floor(v) end
	local x0, y0, width, height, cells = t.x0, t.y0, t.width, t.height, t.cells
	if not (whole(x0) and whole(y0) and whole(width) and whole(height)) or width < 1 or height < 1 then
		return nil, "a terrain grid it cannot read"
	end
	if type(cells) ~= "table" or #cells ~= 2 * width * height then
		return nil, "a terrain grid of " .. width .. " by " .. height .. " cells with " .. tostring(type(cells) == "table"
			and #cells or 0) .. " values"
	end
	if width > capture.TERRAIN_CELLS or width > 65535 then
		return nil, "a stroke " .. width .. " cells wide, wider than the room carries: use a smaller brush"
	end
	local cell, why = capture.terrainCell()
	if not cell then return nil, why end
	local rows = math.floor(capture.TERRAIN_CELLS / width)
	local actions = {}
	for first = 0, height - 1, rows do
		local last = math.min(height, first + rows) - 1
		local band = {}
		for r = first, last do
			for c = 0, width - 1 do
				local i = 2 * (r * width + c)
				band[#band + 1] = { target = cells[i + 1], before = cells[i + 2] }
			end
		end
		actions[#actions + 1] = { Terraform = {
			origin = { x = x0 * cell, y = (y0 + first) * cell },
			cell = cell,
			columns = width,
			cells = band,
		} }
	end
	return actions
end

-- A Paint action (tpf3mp_proto action::TerrainPaint, as the hook made it
-- from the painter's stroke) in one line for the log: its texels, how many
-- it paints (255 leaves one as it is), and its mask.
function capture.paintSummary(t)
	local m = type(t) == "table" and t.materials
	if type(m) ~= "table" or type(m.runs) ~= "table" then return "an unreadable paint stroke" end
	local painted = 0
	for _, r in ipairs(m.runs) do
		if r.value ~= 255 then painted = painted + r.count end
	end
	local mask = "no mask"
	if type(t.mask) == "table" and type(t.mask.runs) == "table" then
		local set = 0
		for _, r in ipairs(t.mask.runs) do
			if r.value ~= 0 then set = set + r.count end
		end
		mask = string.format("a mask with %d set", set)
	end
	return string.format("%d by %d texels from (%d, %d), %d painted, %s",
		m.columns, m.rows, m.x0, m.y0, painted, mask)
end

-- Most assets one PlantAssets action carries: about 40 bytes each, well
-- inside the room's 48 KiB payload. A stroke of more goes in several, each
-- a run of whole groups.
capture.STROKE_ASSETS = 800
-- Most groups one PlantAssets action adds (action::MAX_STROKE_GROUPS), and
-- most assets one group holds (action::MAX_GROUP_ASSETS).
capture.STROKE_GROUPS = 256
capture.GROUP_ASSETS = 512

-- An asset brush stroke, as the hook read it at the click (tpf3mp_native.built:
-- `{ assets = { removed = { entity, ... }, groups = { { { model =, m = { 16
-- numbers } }, ... }, ... } } }`, crates/tpf3mp-hook/src/terrain.rs), as
-- PlantAssets actions (tpf3mp_proto action::AssetStroke): each group the
-- stroke replaces named by its first asset and how many it holds, as the
-- asset bulldozer names one (the groups still stand: the click was stopped),
-- and each group it builds, every asset its model's index in the action's
-- models and its matrix. The replaced groups go with the first action.
-- Returns the list of actions, or nil and why.
function capture.assetStroke(assets)
	if type(assets) ~= "table" or type(assets.groups) ~= "table" or type(assets.removed) ~= "table" then
		return nil, "an asset stroke it cannot read"
	end
	local engine = module("engine")
	local removed = {}
	for _, id in ipairs(assets.removed) do
		local ok, list = pcall(engine.assetsOf, id)
		if not ok then return nil, "a group the stroke replaces does not read: " .. tostring(list) end
		local first = list[1]
		if first == nil then return nil, "a group the stroke replaces holds nothing" end
		removed[#removed + 1] = { first = { model = first.model, at = { x = first.x, y = first.y, z = first.z } },
			count = #list }
	end
	local actions, current, assetsIn = {}, nil, 0
	local function open()
		current = { models = {}, removed = #actions == 0 and removed or {}, groups = {}, index = {} }
		actions[#actions + 1] = current
		assetsIn = 0
	end
	for g, group in ipairs(assets.groups) do
		if type(group) ~= "table" or #group == 0 then return nil, "group " .. g .. " of the stroke holds nothing" end
		if #group > capture.GROUP_ASSETS then
			return nil, "a group of " .. #group .. " assets, more than the room carries (" .. capture.GROUP_ASSETS .. ")"
		end
		if current == nil or assetsIn + #group > capture.STROKE_ASSETS or #current.groups >= capture.STROKE_GROUPS then
			open()
		end
		local placed = {}
		for _, a in ipairs(group) do
			local file = engine.modelFile(a.model)
			if file == nil then return nil, "an asset of no model in group " .. g end
			local index = current.index[file]
			if index == nil then
				if #current.models >= 64 then return nil, "a stroke of more than 64 kinds of asset" end
				current.models[#current.models + 1] = file
				index = #current.models - 1
				current.index[file] = index
			end
			local ok, transform = pcall(capture.transform, a.m)
			if not ok then return nil, "an asset whose matrix does not read: " .. tostring(transform) end
			placed[#placed + 1] = { model = index, transform = transform }
		end
		current.groups[#current.groups + 1] = placed
		assetsIn = assetsIn + #group
	end
	if #actions == 0 then
		if #removed == 0 then return nil, "an asset stroke that changes nothing" end
		open()
	end
	local out = {}
	for i, a in ipairs(actions) do
		out[i] = { PlantAssets = { models = a.models, removed = a.removed, groups = a.groups } }
	end
	return out
end

-- A PlantAssets action in one line for the log.
function capture.assetSummary(t)
	if type(t) ~= "table" or type(t.groups) ~= "table" then return "an unreadable asset stroke" end
	local n = 0
	for _, g in ipairs(t.groups) do n = n + #g end
	return string.format("%d group(s) of %d asset(s), replacing %d group(s)", #t.groups, n, #(t.removed or {}))
end

-- A Terraform action in one line for the log.
function capture.terraformSummary(t)
	if type(t) ~= "table" or type(t.cells) ~= "table" or type(t.columns) ~= "number" or t.columns < 1 then
		return "an unreadable terraform"
	end
	local low, high, changed = nil, nil, 0
	for _, c in ipairs(t.cells) do
		low = math.min(low or c.target, c.target)
		high = math.max(high or c.target, c.target)
		if c.target ~= c.before then changed = changed + 1 end
	end
	local cell = t.cell or 0
	return string.format("%d by %d cells of %g m from cell (%g, %g), %d changed, heights %.2f to %.2f m",
		t.columns, #t.cells / t.columns, cell, cell > 0 and t.origin.x / cell or 0,
		cell > 0 and t.origin.y / cell or 0, changed, low or 0, high or 0)
end

-- A stop placed on a street or track with the stop tool (tpf3mp_proto
-- action::PlaceStop), read off its proposal by tpf3mp/engine.lua. Returns
-- the action table; false for a proposal of nothing; or nil and why.
--
-- Transport Fever 3's proposal does not name the stop (build 40408: its
-- edge objects carry no model), which is a construction the construction
-- menu gave the tool; the GUI notes it (capture.STOP_NOTE,
-- gui/tpf3mp/gui_state.script.lua) and `link` reads the note.
capture.STOP_NOTE = "stop-tool"
-- Whether the signal the tool places is one-way: "1" or "0".
capture.ONE_WAY_NOTE = "stop-tool-one-way"
-- The settings the tool builds the stop or signal with (its
-- EdgeObjectBuilder.params), with its construction, as capture.paramsNote
-- writes them.
capture.PARAMS_NOTE = "stop-tool-params"
-- The tool the construction menu last started: its action and resource.
capture.TOOL_NOTE = "tool"

-- In a GUI Lua state: notes the stop the construction menu gives the stop
-- tool, for capture.stop, which runs in another. The menu makes the tool's
-- action with construction_react_util.getActionParams (`util`), whose
-- EdgeObjectBuilder names the stop's construction (resName; build 40408,
-- gui/construction/construction_react_util.tl); each call is noted through
-- `link` (tpf3mp/bridge.lua). Once a state. Returns whether it watches.
function capture.watchStopTool(util, link)
	if type(package) == "table" and type(package.loaded) == "table" then
		if package.loaded["tpf3mp.stopToolWatched"] then return true end
	end
	if type(util) ~= "table" or type(util.getActionParams) ~= "function" or link == nil then return false end
	local original = util.getActionParams
	util.getActionParams = function(definition, ...)
		local result = original(definition, ...)
		-- The tool picked, for the log (capture.TOOL_NOTE).
		pcall(function()
			link:note(capture.TOOL_NOTE, tostring(definition.action) .. " " .. tostring(definition.resName))
		end)
		pcall(function()
			local builder = result.constructionActionParams.edgeObjectBuilder
			local name = builder and builder.resName
			if type(name) == "string" and name ~= "" then
				local params
				local read = pcall(function() params = builder.params end)
				link:note(capture.PARAMS_NOTE, capture.paramsNote(name, read and params or nil, read))
				link:note(capture.STOP_NOTE, name)
				link:note(capture.ONE_WAY_NOTE, builder.oneWay == true and "1" or "0")
			end
		end)
		return result
	end
	if type(package) == "table" and type(package.loaded) == "table" then
		package.loaded["tpf3mp.stopToolWatched"] = true
	end
	return true
end

-- A note is at most 512 bytes (tpf3mp_native.note), and a longer one is cut
-- short: the settings' note is written whole or not at all.
capture.MAX_NOTE = 500
-- What separates the fields of the settings' note.
local SEP = "\t"

-- The note of a stop tool's settings: "1", its construction, how many
-- settings, then each as key=<i|f|b><value>, tab-separated, keys sorted;
-- or "!" and why, for settings it cannot carry (`read` false: they did not
-- read).
function capture.paramsNote(name, params, read)
	if read == false then return "!the tool's settings did not read" end
	local ok, flat = pcall(module("engine").flatParams, params)
	if not ok then return "!" .. tostring(flat) end
	local parts = { "1", name, tostring(#flat) }
	for _, p in ipairs(flat) do
		local v = p.value
		local text
		if v.Int ~= nil then text = "i" .. string.format("%.0f", v.Int)
		elseif v.Fixed ~= nil then text = "f" .. string.format("%.17g", v.Fixed)
		else text = "b" .. (v.Bool and "1" or "0") end
		parts[#parts + 1] = p.key .. "=" .. text
	end
	local note = table.concat(parts, SEP)
	if #note > capture.MAX_NOTE or name:find(SEP, 1, true) then return "!more settings than a note holds" end
	return note
end

-- The settings a paramsNote wrote for the construction `name`, as the
-- schema's list; nil and why where it is missing, cut short, another
-- construction's or one the tool could not carry.
function capture.readParamsNote(note, name)
	if type(note) ~= "string" or note == "" then return nil, "the tool's settings were not noted" end
	if note:sub(1, 1) == "!" then return nil, note:sub(2) end
	local parts = {}
	for part in (note .. SEP):gmatch("([^" .. SEP .. "]*)" .. SEP) do parts[#parts + 1] = part end
	if parts[1] ~= "1" or parts[2] ~= name then return nil, "the tool's settings are another construction's" end
	local count = tonumber(parts[3])
	if count == nil or count ~= #parts - 3 then return nil, "the tool's settings were cut short" end
	local out = {}
	for i = 4, #parts do
		local key, kind, text = parts[i]:match("^([%a_][%w_%.]*)=([ifb])(.*)$")
		local value
		if kind == "i" and text:match("^%-?%d+$") then value = { Int = tonumber(text) }
		elseif kind == "f" and tonumber(text) ~= nil then value = { Fixed = tonumber(text) }
		elseif kind == "b" and (text == "1" or text == "0") then value = { Bool = text == "1" } end
		if key == nil or value == nil then return nil, "the tool's settings did not read" end
		if #out > 0 and out[#out].key >= key then return nil, "the tool's settings did not read" end
		out[#out + 1] = { key = key, value = value }
	end
	return out
end

-- Signals a script places along tracks after its player's signal (Auto
-- Signals; tpf3mp/modbuild.lua): a PlaceSignals action.
function capture.signals(simple)
	return module("engine").placeSignals(simple)
end

function capture.stop(proposal, link)
	local noted = link and link.note and link:note(capture.STOP_NOTE) or nil
	local oneWay = link and link.note and link:note(capture.ONE_WAY_NOTE) == "1"
	local params, why
	if noted then params, why = capture.readParamsNote(link:note(capture.PARAMS_NOTE), noted) end
	return module("engine").placeStop(proposal, noted, oneWay, params, why)
end

-- The bulldozer's removal (tpf3mp_proto action::Bulldoze), read off its
-- proposal by tpf3mp/engine.lua: a construction (a town building among
-- them), edges with the town buildings the game removes along them, or a
-- stop. Returns the action table; false for a proposal of nothing; or nil
-- and why.
--
-- A proposal that removes a construction and adds one is an edit: a module
-- taken off with the module bulldozer, if that reaches game scripts as the
-- bulldozer's (INFERRED, not seen in the game), is carried as the edit it is
-- (capture.construction), or refused. One that removes something that is no
-- construction and adds a construction of no file is the asset
-- bulldozer's (trees and other assets: their group rebuilt without the ones
-- removed), as is one that removes an asset group alone (its last assets
-- taken): carried behind TPF3MP_TREE_BULLDOZE=1 (engine.captureAssets),
-- else refused with what it removes (tpf3mp/engine.lua, notConstruction).
function capture.bulldoze(proposal)
	local toRemove = get(proposal, "toRemove")
	if (length(get(proposal, "toAdd")) or 0) > 0 and (length(toRemove) or 0) > 0 then
		for i = 1, length(toRemove) do
			local entity = get(toRemove, i)
			local c = api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION)
			if c == nil then
				local engine = module("engine")
				-- Trees and other assets: carried where the hook lets
				-- them (TPF3MP_TREE_BULLDOZE=1), for a trial of the replay.
				local okA, asset = pcall(api.engine.getComponent, entity, api.type.ComponentType.ASSET_GROUP)
				if okA and asset ~= nil and engine.treesOn() then return engine.captureAssets(proposal) end
				return nil, engine.notConstruction(entity)
			end
			if (length(get(c, "townBuildings")) or 0) == 0 then return capture.construction(proposal) end
		end
		return nil, "a bulldozer proposal that builds"
	end
	-- The last assets of a group taken: the asset bulldozer removes the
	-- group and adds nothing (CreateProposalAddAsset with no model kept).
	if (length(get(proposal, "toAdd")) or 0) == 0 and (length(toRemove) or 0) == 1 then
		local engine = module("engine")
		local okA, asset = pcall(api.engine.getComponent, get(toRemove, 1), api.type.ComponentType.ASSET_GROUP)
		if okA and asset ~= nil and engine.treesOn() then return engine.captureAssets(proposal) end
	end
	return module("engine").bulldoze(proposal)
end

-- A build a window sends itself (api.cmd.makeWorldBuildProposalCmd, as
-- tpf3mp/guard.lua's CARRY takes it): the room carries an edit of one
-- construction, as the construction menu's parameters and the station's
-- cargo buttons make one (api.engine.util.proposal
-- .createProposalReplaceConstruction, gui/construction/construction.tl and
-- gui/entity_window/entity_window_util.tl, build 40408). Every other build
-- from a window stays refused. Returns the action table, or raises why not.
--
-- A window's build that rebuilds edges in place, nothing else (no
-- construction, no node added or removed, every new edge between the ends
-- of one it replaces): the bridge and tunnel window's type
-- (gui/entity_window/bridge_and_tunnel.tl, api.engine.util.proposal
-- .createBridgeOrTunnelProposal, build 40408) makes one. It is carried as
-- the road and track modifiers' rebuild is (capture.modify) while
-- acceptance.lua's `bridges` is on (since a two-player game on build 40420,
-- 2026-10-11, showed the window's proposal reads as the modifiers' does);
-- with it off, it is refused, saying so.
function capture.inPlace(proposal)
	local p = get(proposal, "proposal")
	if p == nil or (length(get(proposal, "toAdd")) or 0) > 0 or (length(get(proposal, "toRemove")) or 0) > 0 then
		return false
	end
	if (length(get(p, "addedNodes")) or 0) > 0 or (length(get(p, "removedNodes")) or 0) > 0 then return false end
	local added, removed = get(p, "addedSegments"), get(p, "removedSegments")
	local n = length(added)
	if n == nil or n == 0 or length(removed) ~= n then return false end
	local used = {}
	for i = 1, n do
		local a = get(get(added, i), "comp")
		local a0, a1 = get(a, "node0"), get(a, "node1")
		local found = nil
		for k = 1, n do
			local r = get(get(removed, k), "comp")
			local r0, r1 = get(r, "node0"), get(r, "node1")
			if not used[k] and ((r0 == a0 and r1 == a1) or (r0 == a1 and r1 == a0)) then
				found = k
				break
			end
		end
		if found == nil then return false end
		used[found] = true
	end
	return true
end

function capture.windowBuild(_ctx, proposal)
	if capture.inPlace(proposal) then
		if module("acceptance").bridges ~= true then
			error("rebuilding a bridge or tunnel from its window awaits two-player game acceptance", 0)
		end
		local action, why = capture.modify(proposal)
		if not action then error(why or "a window's rebuild of nothing", 0) end
		return action
	end
	local p = proposal and proposal.proposal
	if p and #(proposal.toAdd or {}) == 0 and #(proposal.toRemove or {}) == 0
		and ((p.nodeConfigsToAdd and #p.nodeConfigsToAdd > 0)
		or (p.nodeConfigsToRemove and #p.nodeConfigsToRemove > 0)) then
		return capture.junction(proposal)
	end
	local action, why = capture.construction(proposal)
	if not action then error(why, 0) end
	if action.BuildConstruction.replaces == nil then error("building from this window", 0) end
	return action
end

-- A proposal's street part in one line, for the log (tpf3mp/engine.lua);
-- "" when it has none.
function capture.describe(proposal)
	return module("engine").describe(proposal)
end

-- What a tool changed of the edges it rebuilt, for the log
-- (tpf3mp/engine.lua).
function capture.rebuildDiff(proposal)
	return module("engine").rebuildDiff(proposal)
end

-- ------------------------------------------------------ vehicles and lines
--
-- The vehicle and line windows' commands (api.cmd.make*Cmd, with the
-- arguments the windows give them) as actions: tpf3mp/guard.lua's CARRY.
-- `ctx` names what a command names by entity:
--
--   ctx.vehicle(e), ctx.line(e), ctx.group(e) -> canonical id, or nil
--                                               (tpf3mp/registry.lua)
--   ctx.depot(e) -> { file =, at = { x, y, z } } of the depot's
--                   construction, and the depot's index among its
--                   depots from 0 (capture.depotRef); or nil, nil, why
--   ctx.model(id) -> a vehicle model's file name, or nil
--   ctx.parts(e) -> a vehicle's parts, front to back, each
--                   { model = modelId, purchased = purchaseTime }, or nil
--   ctx.town(e)   -> a town's canonical id, or nil
--   ctx.player()  -> the player's company entity, or nil
--
-- Each returns the action table, or raises why the room cannot carry it.

local function named(what, id)
	if id == nil then error(what, 0) end
	return id
end

local function tintOf(v)
	local r, g, b = get(v, "x"), get(v, "y"), get(v, "z")
	if r == nil then r, g, b = get(v, 1), get(v, 2), get(v, 3) end
	if type(r) ~= "number" or type(g) ~= "number" or type(b) ~= "number" then
		error("a colour it cannot read", 0)
	end
	return { r = r, g = g, b = b }
end

local function each(list, fn)
	local out = {}
	for i = 1, (length(list) or 0) do out[i] = fn(get(list, i)) end
	return out
end

local function vehicleOf(ctx, entity)
	return named("a vehicle the room cannot name", ctx.vehicle(entity))
end

local function lineOf(ctx, entity)
	return named("a line the room cannot name", ctx.line(entity))
end

-- One TransportVehiclePart of a vehicle config as the schema's ConsistPart.
-- Each part's reversed flag rides along: a turned wagon (an ICE's tail head,
-- a cab car) stays turned (TPF2-MP learned it the hard way, release
-- 0.6.1.12, from tearded's fork).
local function consistPart(ctx, tvp)
	local part = get(tvp, "part")
	return {
		model = named("a vehicle model the room cannot name", ctx.model(get(part, "modelId"))),
		reversed = get(part, "reversed") == true,
		loads = each(get(part, "compartment2loadConfig"), function(lc)
			return { config = get(lc, "loadConfigIndex"), cargo = get(lc, "cargoTypeId") }
		end),
		color = tintOf(get(part, "color")),
	}
end

-- A construction's depots, in the order every game lists them: its
-- CONSTRUCTION component's `depots`, then each of its subconstructions that
-- is itself a depot (a VEHICLE_DEPOT) and not listed yet. In build 40408 a
-- depot is its construction's subconstruction: a road, rail or ship depot's
-- own (depots/rail/rail_depot.script.lua, `subconstructions = { depot }`),
-- and an airfield's or airport's hangar module's
-- (stations/air/airfield/af_hangar.module.lua and
-- airport/ap_hangar.module.lua: a subconstruction with a `depot`); the
-- game's store buys at that entity (gui/line_vehicle_mgmt/
-- vehicle_react_util.tl, makeVehicleBuyCmd(player, depotEntity, config)),
-- and the construction window finds it among `subconstructions`
-- (gui/entity_window/make_entity_window.tl). An airfield or airport built
-- without a hangar module has no depot at all, and a harbour never has one:
-- ships are bought at a ship depot. Read from the construction alone, the
-- same in every game. `comp` is the construction's CONSTRUCTION component.
function capture.depotsOf(api, comp)
	local out, seen = {}, {}
	local function add(e)
		if type(e) == "number" and e >= 0 and not seen[e] then
			seen[e] = true
			out[#out + 1] = e
		end
	end
	local depots = get(comp, "depots")
	for k = 1, (length(depots) or 0) do add(get(depots, k)) end
	local ok, VEHICLE_DEPOT = pcall(function() return api.type.ComponentType.VEHICLE_DEPOT end)
	if ok and VEHICLE_DEPOT ~= nil then
		local subs = get(comp, "subconstructions")
		for k = 1, (length(subs) or 0) do
			local e = get(subs, k)
			if type(e) == "number" and not seen[e] then
				local found, d = pcall(api.engine.getComponent, e, VEHICLE_DEPOT)
				if found and d ~= nil then add(e) end
			end
		end
	end
	return out
end

-- A depot as actions name one (action::ConstructionRef): its construction's
-- file and place, and the depot's index among that construction's depots
-- (capture.depotsOf), from 0. The construction is the one the street
-- connector names for the depot, else the one it names for the depot as a
-- subconstruction (an airfield's hangar: no street reaches it), else the one
-- construction whose depots list it. Returns { file =, at = { x, y, z } }
-- and the index; or nil, nil and why, failing closed: a depot no
-- construction lists, one two constructions list, or one its construction
-- does not list is never guessed at (the first depot used to be).
function capture.depotRef(api, depot)
	local ok, CONSTRUCTION = pcall(function() return api.type.ComponentType.CONSTRUCTION end)
	if not ok or CONSTRUCTION == nil then return nil, nil, "no construction component to read" end
	local function constructionFor(kind)
		local con
		pcall(function()
			local e = api.engine.system.streetConnectorSystem[kind](depot)
			if type(e) == "number" and e >= 0 then con = e end
		end)
		return con
	end
	local c
	local con = constructionFor("getConstructionEntityForDepot")
		or constructionFor("getConstructionEntityForSubconstruction")
	if con ~= nil then
		pcall(function() c = api.engine.getComponent(con, CONSTRUCTION) end)
	else
		local listing = {}
		pcall(function()
			local list = api.engine.getEntitiesWithComponent(CONSTRUCTION)
			for i = 1, #list do
				local e = list[i]
				local comp = api.engine.getComponent(e, CONSTRUCTION)
				if comp ~= nil then
					for _, d in ipairs(capture.depotsOf(api, comp)) do
						if d == depot then listing[#listing + 1] = comp break end
					end
				end
			end
		end)
		if #listing > 1 then return nil, nil, "a depot " .. #listing .. " constructions list" end
		c = listing[1]
	end
	if c == nil then return nil, nil, "a depot no construction lists" end
	local t = get(c, "transf")
	local file, x, y, z = get(c, "fileName"), get(t, 13), get(t, 14), get(t, 15)
	if type(file) ~= "string" or file == "" or type(x) ~= "number" or type(y) ~= "number" or type(z) ~= "number" then
		return nil, nil, "a depot whose construction it cannot read"
	end
	-- Which of its depots (an airport's second hangar).
	for k, d in ipairs(capture.depotsOf(api, c)) do
		if d == depot then return { file = file, at = { x = x, y = y, z = z } }, k - 1 end
	end
	return nil, nil, "a depot " .. file .. " does not list among its depots"
end

-- What the log says of a purchase's depot: the entity the store passed, its
-- owner, and the construction and index the room names it by (or why it
-- cannot), so a vehicle that leaves another depot than the player meant
-- shows which one the store chose.
function capture.depotText(depot, owner, ref, index, why)
	local whose = type(owner) == "number" and ("owned by " .. string.format("%d", owner)) or "owned by no one"
	if ref == nil then
		return string.format("the store buys at depot entity %s (%s), which the room cannot name: %s",
			tostring(depot), whose, tostring(why))
	end
	return string.format("the store buys at depot entity %s (%s): depot %d of %s at (%.1f, %.1f, %.1f)",
		tostring(depot), whose, index or -1, tostring(ref.file), ref.at.x, ref.at.y, ref.at.z)
end

-- The depot's store: a vehicle config (TransportVehicleConfig) bought there.
function capture.vehicleBuy(ctx, _player, depot, config)
	local ref, index, why = ctx.depot(depot)
	if type(ctx.say) == "function" then
		local owner = type(ctx.owner) == "function" and ctx.owner(depot) or nil
		pcall(ctx.say, capture.depotText(depot, owner, ref, index, why))
	end
	if ref == nil then
		error("a depot the room cannot name" .. (why and (": " .. tostring(why)) or ""), 0)
	end
	if type(index) ~= "number" or index < 0 or index > 255 then
		error("a depot the room cannot name: its construction has more than 256 depots", 0)
	end
	return { BuyVehicle = {
		depot = ref,
		depot_index = index,
		consist = each(get(config, "vehicles"), function(tvp) return consistPart(ctx, tvp) end),
		groups = each(get(config, "vehicleGroups"), function(n) return n end),
		multiple_units = each(get(config, "muFileNames"), function(name) return name end),
	} }
end

-- The vehicle window's "modify" and the store's "replace" (build 40408,
-- gui/line_vehicle_mgmt/vehicle_react_util.tl HandleVehicleChanges): one
-- makeVehicleReplaceCmd per vehicle, a group's vehicles one by one, with the
-- config the store built. A part the player left in the consist is the
-- vehicle's own, its purchase time kept; the store bought the rest, with
-- purchase time 0, which HandleVehicleChanges sets to the GUI's game time
-- before it sends. So a part is kept when it is one of the vehicle's own
-- parts, of the same model and purchase time, each own part matched once,
-- front to back. `ctx.parts(e)` lists the vehicle's parts now, each
-- { model = modelId, purchased = purchaseTime }.
function capture.vehicleReplace(ctx, vehicle, config)
	local id = vehicleOf(ctx, vehicle)
	local own = ctx.parts and ctx.parts(vehicle)
	if type(own) ~= "table" then error("a vehicle whose parts the room cannot read", 0) end
	local taken = {}
	local consist = each(get(config, "vehicles"), function(tvp)
		local out = { part = consistPart(ctx, tvp) }
		local model, purchased = get(get(tvp, "part"), "modelId"), get(tvp, "purchaseTime")
		if type(purchased) == "number" and purchased > 0 then
			for i, p in ipairs(own) do
				if not taken[i] and p.model == model and p.purchased == purchased then
					taken[i], out.kept = true, i - 1
					break
				end
			end
		end
		return out
	end)
	if #consist == 0 then error("a replacement of no vehicles", 0) end
	return { ReplaceVehicle = {
		vehicle = id,
		consist = consist,
		groups = each(get(config, "vehicleGroups"), function(n) return n end),
		multiple_units = each(get(config, "muFileNames"), function(name) return name end),
	} }
end

-- `stopIndex` -1 is the line manager's "Next Reachable Stop" (build 40408):
-- the game picks the stop, which the action carries as no first stop.
function capture.vehicleSetLine(ctx, vehicle, line, stopIndex)
	local first = nil
	if stopIndex ~= -1 then first = stopIndex end
	return { AssignLine = {
		vehicles = { vehicleOf(ctx, vehicle) }, line = lineOf(ctx, line), first_stop = first,
	} }
end

function capture.vehicleSell(ctx, vehicles)
	return { SellVehicle = { vehicles = each(vehicles, function(e) return vehicleOf(ctx, e) end) } }
end

function capture.vehicleStop(ctx, vehicle, stopped)
	return { VehicleOp = { vehicle = vehicleOf(ctx, vehicle), change = { Stop = stopped == true } } }
end

-- Sold on arrival, build 40408 crashes when the vehicle reaches the depot:
-- it sells the vehicle, then asks the vehicle it removed where its depot is
-- (Engine.h:323). TF3's own windows only ever send false.
function capture.vehicleToDepot(ctx, vehicle, sell, jumpTo)
	if jumpTo ~= nil then error("moving a vehicle into a depot at once", 0) end
	if sell == true then error("selling a vehicle when it reaches the depot (the game crashes there)", 0) end
	return { VehicleOp = { vehicle = vehicleOf(ctx, vehicle), change = { ToDepot = { sell = false } } } }
end

function capture.vehicleReverse(ctx, vehicle)
	return { VehicleOp = { vehicle = vehicleOf(ctx, vehicle), change = "Reverse" } }
end

function capture.vehicleDepart(ctx, vehicle)
	return { VehicleOp = { vehicle = vehicleOf(ctx, vehicle), change = "Depart" } }
end

-- Held at its stops until told to leave, or not: what a timetable mod's
-- game script sends (tpf3mp/modguard.lua).
function capture.vehicleManualDeparture(ctx, vehicle, manual)
	return { VehicleOp = { vehicle = vehicleOf(ctx, vehicle), change = { ManualDeparture = manual == true } } }
end

-- A line's waypoint (the game's Waypoint) as the schema's Waypoint: on a
-- street or track, the lane its EdgePos names, in the transport network of
-- a street or track edge (by its ends, node 0 first) or of a construction
-- (by its file and place), and where along it; a ship's or aircraft's in the
-- open by its position. Its tag goes as it is. Raises why the room cannot
-- name one.
function capture.waypoint(w)
	local tag = get(w, "tag")
	if type(tag) ~= "number" or tag ~= math.floor(tag) then error("a waypoint without its tag", 0) end
	local edgePos = get(w, "edgePos")
	local id = edgePos and get(edgePos, "edgeId")
	local entity = id and get(id, "entity")
	if type(entity) == "number" and entity >= 0 then
		local index, param = get(id, "index"), get(edgePos, "param")
		if type(index) ~= "number" or type(param) ~= "number" then error("a waypoint it cannot read", 0) end
		local of
		pcall(function()
			local CT = api.type.ComponentType
			local edge = api.engine.getComponent(entity, CT.BASE_EDGE)
			if edge ~= nil then
				local a = api.engine.getComponent(edge.node0, CT.BASE_NODE).position
				local b = api.engine.getComponent(edge.node1, CT.BASE_NODE).position
				local street = api.engine.getComponent(entity, CT.BASE_EDGE_STREET) ~= nil
				local function xyz(p) return { x = p.x or p[1], y = p.y or p[2], z = p.z or p[3] } end
				of = { Edge = { network = street and "Street" or "Track", ends = { a = xyz(a), b = xyz(b) } } }
			else
				local ref = capture.replaced(api.engine.getComponent(entity, CT.CONSTRUCTION))
				if ref then of = { Construction = ref } end
			end
		end)
		if of == nil then error("a waypoint on a network the room cannot name", 0) end
		return { at = { Lane = { of = of, index = index, param = param } }, tag = tag }
	end
	local p = get(w, "pos")
	local x, y, z = get(p, "x"), get(p, "y"), get(p, "z")
	if type(x) ~= "number" or type(y) ~= "number" or type(z) ~= "number" then
		error("a waypoint with no place", 0)
	end
	return { at = { Open = { x = x, y = y, z = z } }, tag = tag }
end

-- The game's load modes (Line.LoadMode), numbers to the schema's names.
local LOAD_MODES = { [0] = "LoadIfAvailable", [1] = "FullLoadAny", [2] = "FullLoadAll", [3] = "LegacyUnloadOnly" }

-- A Line component as the schema's LineData.
function capture.lineData(ctx, line)
	local stops = each(get(line, "stops"), function(s)
		local waypoints = each(get(s, "waypoints"), capture.waypoint)
		local config = get(s, "stopConfig")
		local mode = tonumber(get(s, "loadMode"))
		return {
			group = named("a station the room cannot name", ctx.group(get(s, "stationGroup"))),
			terminal = { station = get(s, "station"), terminal = get(s, "terminal") },
			alternatives = each(get(s, "alternativeTerminals"), function(a)
				return { station = get(a, "station"), terminal = get(a, "terminal") }
			end),
			load_mode = LOAD_MODES[mode] or error("a load mode " .. tostring(mode), 0),
			min_wait = get(s, "minWaitingTime"),
			max_wait = get(s, "maxWaitingTime"),
			max_extra_wait = get(s, "maxAdditionalWaitingTime"),
			rules = {
				load = each(get(config, "load"), function(b) return b == true end),
				max_load = each(get(config, "maxLoad"), function(f) return f end),
				force_unload = get(config, "forceUnload") == true,
				destroy_for_config_change = get(config, "destroyForConfigChange") == true,
				destroy_for_refresh = get(config, "destroyForRefresh") == true,
			},
			waypoints = waypoints,
		}
	end)
	local info = get(line, "vehicleInfo")
	local modes, transport = {}, info and get(info, "transportModes")
	if type(transport) ~= "table" then error("a line's transport modes it cannot read", 0) end
	for mode, on in pairs(transport) do
		if on == true then modes[#modes + 1] = mode end
	end
	table.sort(modes)
	return {
		stops = stops,
		modes = modes,
		custom_filters = get(line, "customFilters") == true,
		reservation_priority = get(line, "reservationPriority") or 0,
	}
end

function capture.lineCreate(ctx, name, color, _player, line)
	return { CreateLine = { name = name, color = tintOf(color), line = capture.lineData(ctx, line) } }
end

function capture.lineUpdate(ctx, lineEntity, line)
	return { EditLine = { line = lineOf(ctx, lineEntity), change = { Update = capture.lineData(ctx, line) } } }
end

function capture.lineDestroy(ctx, lineEntity)
	return { EditLine = { line = lineOf(ctx, lineEntity), change = "Delete" } }
end

-- ------------------------------------------------------------ prospecting
--
-- The construction menu's prospection (gui/construction/
-- construction_react_util.tl, ProspectionActionRecipe.onSelect): the event
-- `Companies` `spawnIndustry` to the company script, with the player's
-- company, the town picked, the industry types, the permit and the cargo
-- (investigation/TPF3_PROSPECTING_2026-09-30.md). The types keep the order
-- the menu listed them in, which the game's shuffle depends on.
function capture.prospect(ctx, param)
	if type(param) ~= "table" then error("a prospection it cannot read", 0) end
	local player = ctx.player and ctx.player()
	if player == nil or get(param, "companyEntity") ~= player then
		error("prospecting for another company", 0)
	end
	local cargo = get(param, "cargoType")
	if type(cargo) ~= "string" or cargo == "" then error("a prospection for no cargo", 0) end
	local permit = get(param, "permitKey")
	if permit ~= nil and type(permit) ~= "string" then error("a permit it cannot read", 0) end
	local types = get(param, "types")
	local n = length(types)
	if n == nil or n == 0 then error("a prospection that can find no industry", 0) end
	local industries = {}
	for i = 1, n do
		local t = get(types, i)
		if type(t) ~= "string" or t == "" then error("an industry type it cannot read", 0) end
		industries[i] = t
	end
	local town = ctx.town and ctx.town(get(param, "townEntity")) or nil
	return { Prospect = {
		town = named("a town the room cannot name", town),
		cargo = cargo,
		industries = industries,
		permit = permit,
	} }
end

-- ------------------------------------------------------------ company perks
--
-- The construction menu's perk tools (gui/construction/tools/, build
-- 40408): each picks a town or an industry and sends the company script an
-- event, which spends the perk's permit for the player's company and hands
-- the perk on (game_mechanics/company/company.script.tl, handleEvent).

-- The construction an industry the player picked stands in, as the game's
-- industry window finds it (gui/entity_window/industry/industry.tl), and
-- tpf3mp/registry.lua names industries by: a part the game places in no
-- construction stands for itself. Returns the construction, or nil.
function capture.industryConstruction(part)
	local ok, con = pcall(function()
		return api.engine.system.streetConnectorSystem.getConstructionEntityForSubconstruction(part)
	end)
	if not ok or type(con) ~= "number" or con < 0 then con = part end
	return con
end

-- The industry part of construction `con` a perk acts on: its one industry,
-- or the construction itself where it is one. nil where it has none, or
-- more than one, which no id the room carries tells apart. Every game finds
-- it so (tpf3mp/apply.lua, HANDLERS.Perk).
function capture.industryPart(con)
	local CT = api.type.ComponentType
	local ok, c = pcall(function() return api.engine.getComponent(con, CT.CONSTRUCTION) end)
	local parts = ok and c and get(c, "industries") or nil
	local n = length(parts) or 0
	if n == 1 then return get(parts, 1) end
	if n == 0 then
		local isOne, industry = pcall(function() return api.engine.getComponent(con, CT.INDUSTRY) end)
		if isOne and industry ~= nil then return con end
	end
	return nil
end

local function permitOf(param)
	local permit = get(param, "permitKey")
	if permit ~= nil and (type(permit) ~= "string" or permit == "") then error("a permit it cannot read", 0) end
	return permit
end

local function ownCompany(ctx, param, what)
	local player = ctx.player and ctx.player()
	if player == nil or get(param, "companyEntity") ~= player then
		error(what .. " for another company", 0)
	end
end

-- Industry Greenification (industry_greenify_tool.script.tl): the event
-- `Companies` `MakeGreen` with the player's company, the industry picked
-- and the permit, the industry by its id (action::IndustryId).
function capture.greenify(ctx, param)
	if type(param) ~= "table" then error("a greenification it cannot read", 0) end
	ownCompany(ctx, param, "greenifying")
	local part = get(param, "constructionEntity")
	if type(part) ~= "number" then error("greenifying no industry", 0) end
	local con = capture.industryConstruction(part)
	if capture.industryPart(con) ~= part then error("an industry the room cannot name", 0) end
	local industry = ctx.industry and ctx.industry(con) or nil
	return { Perk = { Greenify = {
		industry = named("an industry the room cannot name", industry),
		permit = permitOf(param),
	} } }
end

-- What the marketing tool charges for a campaign in `year`: the tool's own
-- price (marketing_campaign_tool.script.tl, GetMarketingCost, build 40408),
-- with the game's math helpers (scripts/mathutil.lua: round, mapClamp)
-- written out. The tool books it once the campaign started; the room's
-- action carries it, so every game books the same sum.
capture.MARKETING_COST = 10000000
function capture.marketingCost(year)
	local function round(x) return math.floor(x + .5) end
	local lo, hi = math.log(0.4) / math.log(2), math.log(2.5) / math.log(2)
	local mapped = lo + (hi - lo) * ((year - 1900) / (2020 - 1900))
	if mapped < lo then mapped = lo elseif mapped > hi then mapped = hi end
	local rounding = round(capture.MARKETING_COST / 10)
	return round(capture.MARKETING_COST * math.pow(2, mapped) / rounding) * rounding
end

-- A marketing campaign (marketing_campaign_tool.script.tl): the event
-- `Companies` `startMarketingCampaign` with the player's company, the town
-- picked, the campaign's terms (the tool's town_marketing metadata) and the
-- permit; and the price the tool books after it (capture.marketingCost).
function capture.marketing(ctx, param)
	if type(param) ~= "table" then error("a marketing campaign it cannot read", 0) end
	ownCompany(ctx, param, "marketing")
	local terms = get(param, "marketingParams")
	local duration, factor = get(terms, "durationMs"), get(terms, "lineCostFactor")
	if type(duration) ~= "number" or duration ~= math.floor(duration) or duration < 0
		or type(factor) ~= "number" then
		error("a campaign whose terms it cannot read", 0)
	end
	local ok, year = pcall(function() return api.engine.util.getYear() end)
	if not ok or type(year) ~= "number" then error("a campaign it cannot price", 0) end
	local town = ctx.town and ctx.town(get(param, "townEntity")) or nil
	return { Perk = { Marketing = {
		town = named("a town the room cannot name", town),
		duration_ms = duration,
		line_cost_factor = factor,
		permit = permitOf(param),
		cost = capture.marketingCost(year),
	} } }
end

-- ------------------------------------------------------ town buildings
--
-- The construction a town building stands in, and the building's place in
-- its list of town buildings, from 1 (the construction lists them,
-- api/tealdef/api/engine.d.tl, Construction.townBuildings): the
-- construction the game names for the building as a subconstruction,
-- or the building itself, or else the one construction that lists it.
-- INFERRED: a town building's window names the TOWN_BUILDING entity, which
-- its construction lists. Returns the construction's component and the
-- place, or nil.
function capture.townBuildingOf(entity)
	local CONSTRUCTION = api.type.ComponentType.CONSTRUCTION
	local function lists(con)
		local ok, c = pcall(function() return api.engine.getComponent(con, CONSTRUCTION) end)
		local buildings = ok and c and get(c, "townBuildings") or nil
		for i = 1, (length(buildings) or 0) do
			if get(buildings, i) == entity then return c, i end
		end
		return nil
	end
	local ok, con = pcall(function()
		return api.engine.system.streetConnectorSystem.getConstructionEntityForSubconstruction(entity)
	end)
	if ok and type(con) == "number" and con >= 0 then
		local c, i = lists(con)
		if c then return c, i end
	end
	local c, i = lists(entity)
	if c then return c, i end
	local listed, all = pcall(function() return api.engine.getEntitiesWithComponent(CONSTRUCTION) end)
	for k = 1, (listed and length(all) or 0) do
		c, i = lists(get(all, k))
		if c then return c, i end
	end
	return nil
end

-- A town building's Historic Preservation checkbox (gui/entity_window/
-- town_building/town_building.tl, HistoricBuildingCard): the building by
-- its construction's file and place and its index there
-- (action::Preservation).
function capture.preserve(_ctx, entity, preserved)
	if type(preserved) ~= "boolean" then error("a preservation it cannot read", 0) end
	local c, i = capture.townBuildingOf(entity)
	local ref = c and capture.replaced(c) or nil
	if ref == nil or i > 256 then error("a town building the room cannot name", 0) end
	return { Preserve = { building = ref, index = i - 1, preserved = preserved } }
end

-- Answering a subsidy offer: the subsidy window's Accept or Decline
-- (game_mechanics/subventions/subventions_gui.tl sends the subsidy script
-- `onAccept` or `onDecline` with { uid }), as the offer by its number and
-- its kind, which `ctx.subsidy(uid)` reads from the subsidy script's offers
-- as this game has them. Every game checks the offer again when the room
-- orders it (tpf3mp/companies.lua, acceptSubsidy).
function capture.subsidy(ctx, name, param)
	local uid = get(param, "uid")
	if type(uid) ~= "number" or uid ~= math.floor(uid) then error("a subsidy by no number", 0) end
	local kind = ctx.subsidy and ctx.subsidy(uid) or nil
	if type(kind) ~= "string" or kind == "" then error("a subsidy no longer offered", 0) end
	local ref = { uid = uid, kind = kind }
	if name == "onAccept" then return { Subsidy = { Accept = ref } } end
	return { Subsidy = { Decline = ref } }
end

-- Renaming and recolouring: lines, and the room's companies (the game's
-- company window renames the player's company by its player entity,
-- game_mechanics/company/company.tl), which every game checks is the
-- player's own (tpf3mp/companies.lua).
-- Anything else an entity window's title renames (gui/entity_window/
-- view_manager.tl renames whatever entity the window shows), and the line
-- manager's vehicle names: a vehicle, a station group or a town by its
-- canonical id, any other construction by its file and place (action::
-- Renamed). Every game checks the acting company may (tpf3mp/apply.lua).
function capture.setName(ctx, entity, name)
	local company = ctx.company and ctx.company(entity)
	if company ~= nil then return { CompanyOp = { Rename = { company = company, name = name } } } end
	local line = ctx.line(entity)
	if line ~= nil then return { EditLine = { line = line, change = { Rename = name } } } end
	local what
	local vehicle = ctx.vehicle and ctx.vehicle(entity)
	local group = vehicle == nil and ctx.group and ctx.group(entity) or nil
	local town = vehicle == nil and group == nil and ctx.town and ctx.town(entity) or nil
	if vehicle ~= nil then
		what = { Vehicle = vehicle }
	elseif group ~= nil then
		what = { Station = group }
	elseif town ~= nil then
		what = { Town = town }
	else
		local ok, c = pcall(function()
			return api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION)
		end)
		local ref = ok and c ~= nil and capture.replaced(c) or nil
		if ref == nil then error("renaming this", 0) end
		what = { Construction = ref }
	end
	return { Rename = { what = what, name = name } }
end

-- Recolouring: a line, the room's company, or a vehicle (the vehicle
-- window's and the line manager's colour buttons, VehicleChange::Recolor).
function capture.setColor(ctx, entity, color)
	local company = ctx.company and ctx.company(entity)
	if company ~= nil then return { CompanyOp = { Recolor = { company = company, color = tintOf(color) } } } end
	local line = ctx.line(entity)
	if line ~= nil then return { EditLine = { line = line, change = { Recolor = tintOf(color) } } } end
	local vehicle = ctx.vehicle and ctx.vehicle(entity)
	if vehicle ~= nil then
		return { VehicleOp = { vehicle = vehicle, change = { Recolor = tintOf(color) } } }
	end
	error("recolouring this", 0)
end

return capture
