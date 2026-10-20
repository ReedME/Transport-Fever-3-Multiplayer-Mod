-- tpf3mp/lanes.lua -- the world's lanes, as the game script reads them at a
-- checkpoint (docs/HOOKS.md, "The world's lanes").
--
-- A lane is one part of the world summed up in a short text: the same text
-- on two games means that part of their worlds is the same. The mod's game
-- script reads rolling observations in each postUpdate, accumulating a
-- window that ends at a checkpoint step, and hands them to the hook, which
-- reports their digests to the room; the room compares them between
-- players. The numbers follow the regression harness's model
-- (crates/tpf3mp-testkit/src/regress/model.rs, `lane`), with two more.
--
-- What each lane reads, from the engine the game script runs in:
--
-- - NETWORK: every street and track edge, by its ends to 0.1 m and its road
--   template, lane settings and portable junction configurations; and
--   every level crossing's type, by its nodes' places (tpf3mp/crossings.lua);
-- - CONSTRUCTIONS: every construction, by its file and position to 0.1 m;
-- - LINES: every line's number of stops;
-- - VEHICLES: each vehicle's state, stop, and place on its path (edge and
--   distance to 1 cm, speed to 1 cm/s): the simulation's own state
--   (MOVE_PATH.dyn). Not its position in the world: getPosition, and the
--   path state as the frame began (dyn0), differed between two games in
--   the same simulation update by millimetres (build 40408), the frames
--   being their own; the simulation's state did not;
-- - ECONOMY: the save's own player's balance; with more than one company
--   in the room's roster, each company's balance by its id; and where the
--   game has the subsidy script, its offers, taken, completed and failed
--   subsidies, each with its terms (tpf3mp/subsidies.lua, summary), so two
--   games whose offers differ split here at the next checkpoint;
-- - TOWNS: each town's number of buildings;
-- - PEOPLE: the number of people.
--
-- Nothing is read by an entity's id where an id could differ between two
-- games that agree on the world, except where the save carries it (towns and
-- players). The diagnostic full reader represents a failed lane as "err";
-- the rolling room reader instead refuses failed reads and holds the game.
--
-- The engine lists the entities of some components only
-- (getEntitiesWithComponent refuses BASE_EDGE, LINE and PLAYER on build
-- 40408: "Cannot loop over this component type"), so edges come from the
-- street system's node map, lines from the line system, and the player
-- from the engine's util.
--
-- The full synchronous reader remains for diagnostics and stand-in engines.
-- A lane can also be dumped (`lanes.dump`): its full text, entry by entry,
-- read by the same reader that sums it up, with the raw values it rounds
-- (`%.17g`), keyed by the registry's id where there is one (vehicle-N,
-- line-N, town-N, industry-N) and else by the row it hashes, sorted the
-- same on every game that has the same world; then its summary, the text
-- the hook hashes. The hook writes each entry to hook.log as
-- `lane <n> step <step> <entry>`, for tools/lane_diff.py to diff between
-- games (docs/HOOKS.md, "Lane dumps"). Reading a lane for its digest builds
-- none of it. The network lane's dump can be cut to a box: only the edges
-- with an end inside it, and no junctions.
--
-- `lanes.watch` reads one entity for the edge watch (docs/HOOKS.md, "The
-- edge watch"): an edge's nodes, ends and tangents, and its nodes'
-- positions, at full precision; a node's position.
--
-- Pure Lua over the `api` it is given; the tests hand it a fake.

local lanes = {}
local junctions, subsidies, crossings
if type(ug_require) == "function" then
	junctions = ug_require("tpf3mp_1::/scripts/tpf3mp/junctions.lua")
	subsidies = ug_require("tpf3mp_1::/scripts/tpf3mp/subsidies.lua")
	crossings = ug_require("tpf3mp_1::/scripts/tpf3mp/crossings.lua")
else
	junctions = require("tpf3mp.junctions")
	subsidies = require("tpf3mp.subsidies")
	crossings = require("tpf3mp.crossings")
end

lanes.NETWORK = 0
lanes.CONSTRUCTIONS = 1
lanes.LINES = 2
lanes.VEHICLES = 3
lanes.ECONOMY = 4
lanes.TOWNS = 5
lanes.PEOPLE = 6

-- A pure-Lua hash (tools/probe's): the same on every Lua and platform.
local M1, A1 = 2147483647, 48271
local M2, A2 = 2147483629, 40692
local function hashStr(s)
	local h1, h2 = 2166136261 % M1, 2166136261 % M2
	for i = 1, #s do
		local b = string.byte(s, i)
		h1 = (h1 * A1 + b) % M1
		h2 = (h2 * A2 + b) % M2
	end
	return string.format("%010d-%010d", h1, h2)
end

-- The same hash from the hook, `tpf3mp_native.hash`
-- (crates/tpf3mp-hook/src/lanehash.rs), which does not loop over every byte
-- in Lua; this Lua's own where the state has no hook.
local function fastHash(s)
	local ok, native = pcall(function() return tpf3mp_native end)
	if ok and type(native) == "table" and type(native.hash) == "function" then
		local done, hash = pcall(native.hash, s)
		if done and type(hash) == "string" then return hash end
	end
	return hashStr(s)
end

-- The diagnostic clock in seconds, or nil where the state has none. For the
-- lanes' cost in the log only: nothing read from it reaches the world.
local function clock()
	local readClock = type(os) == "table" and os.clock
	if type(readClock) ~= "function" then return nil end
	local ok, t = pcall(readClock)
	if ok and type(t) == "number" then return t end
	return nil
end

-- What the last lanes.read cost, in seconds: by lane, and the summaries'
-- sorting and hashing (inside the lanes' times).
lanes.cost = { lanes = {}, sort = 0, hash = 0, bytes = 0 }

-- A sorted list's count and hash.
local function summary(rows)
	local t0 = clock()
	table.sort(rows)
	local text = table.concat(rows, "\30")
	local t1 = clock()
	local hash = fastHash(text)
	local t2 = clock()
	if t0 and t1 and t2 then
		lanes.cost.sort = lanes.cost.sort + (t1 - t0)
		lanes.cost.hash = lanes.cost.hash + (t2 - t1)
	end
	lanes.cost.bytes = lanes.cost.bytes + #text
	return #rows .. ":" .. hash
end

local function q01(v) return math.floor((v or 0) * 10 + 0.5) / 10 end

local function vec01(p)
	return string.format("%s,%s,%s", q01(p.x or p[1]), q01(p.y or p[2]), q01(p.z or p[3] or 0))
end

local function entities(api, kind)
	local list = api.engine.getEntitiesWithComponent(api.type.ComponentType[kind])
	local out = {}
	for _, e in pairs(list) do out[#out + 1] = e end
	return out
end

local function component(api, entity, kind)
	return api.engine.getComponent(entity, api.type.ComponentType[kind])
end

-- A field only a dump reads, or nil: the game's components are userdata,
-- which raise on a field they lack.
local function get(value, key)
	if value == nil then return nil end
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

-- A number at full precision, for a dump; anything else as text.
local function full(v)
	if type(v) == "number" then return string.format("%.17g", v) end
	return tostring(v)
end

-- A vector at full precision, for a dump: a table, or the game's userdata
-- (a Vec3f, whose fields x, y and z read but which has no [1]); anything
-- without numbers as text.
local function vecFull(p)
	if p == nil then return "nil" end
	local x, y, z = get(p, "x"), get(p, "y"), get(p, "z")
	if type(x) ~= "number" and type(p) == "table" then x, y, z = p[1], p[2], p[3] end
	if type(x) ~= "number" then return tostring(p) end
	return full(x) .. "," .. full(y) .. "," .. full(z or 0)
end

-- The base game's town growth script's state, as its own
-- town_cargo_util.getTownCargoState reads it: a function from a town to its
-- { experience, level, ... }, or nil. For a dump only; it changes nothing.
local function townGrowth(api)
	local ok, script = pcall(function()
		local e = api.engine.system.gameScriptSystem.getEntityForGameScript("::/game_mechanics/towns/town_cargo.gs")
		return api.engine.getComponent(e, api.type.ComponentType.GAME_SCRIPT)
	end)
	if not ok or script == nil then return function() return nil end end
	local native, plain = get(script, "state_native"), nil
	return function(town)
		if native ~= nil then
			local found, data = pcall(function()
				local d = native:findPath({ "townState", town })
				return d and d:asTable()
			end)
			if found and data ~= nil then return data end
		end
		if plain == nil then
			local state = get(script, "state")
			plain = type(state) == "table" and type(state.townState) == "table" and state.townState or false
		end
		return plain and plain[town] or nil
	end
end

-- One edge's row: its ends to 0.1 m (the one whose text sorts first
-- first), its road template and its lanes, turned with the edge where its
-- ends were swapped. `net` (optional) counts the lane configs; with `api`,
-- the hook decodes the lanes from an owned copy of the component
-- (tpf3mp_native.laneRows), else this Lua reads each.
function lanes.edgeRow(edge, net, api)
	local a, b = vec01(edge.position0), vec01(edge.position1)
	local reversed = a > b
	if reversed then a, b = b, a end
	local row = a .. ">" .. b .. ":" .. tostring(edge.roadTemplate)
	local native = tpf3mp_native
	local laneText, count
	local copy = api and api.type and api.type.BaseEdge and api.type.BaseEdge.new
	if lanes.decoders ~= false and type(native) == "table" and type(native.laneRows) == "function"
		and type(copy) == "function" then
		-- getComponent returns a borrowed reference. Copy once;
		-- Rust reads the owned snapshot, including nested vectors.
		laneText, count = native.laneRows(copy(edge), reversed)
	end
	if laneText == nil then
		local laneRows, configs = {}, edge.laneConfigs
		count = #configs
		for i = 1, count do
			local l, modes = configs[i], {}
			local transportModes = l.transportModes
			for m = 0, 15 do modes[m + 1] = transportModes[m] == true and "1" or "0" end
			laneRows[#laneRows+1] = string.format("%.3f/%.3f/%.3f/%.3f/%s/%s", l.speed,l.width,l.height,
				l.offset * (reversed and -1 or 1), tostring(l.forward ~= reversed), table.concat(modes))
		end
		table.sort(laneRows)
		laneText = table.concat(laneRows,";")
	end
	if net then net.laneConfigs = net.laneConfigs + count end
	return row .. "|lanes:" .. laneText
end

-- Each reader returns its lane's text. With `emit` (a dump), it also calls
-- emit(kind, entity, row, fields) for every row it hashes: the registry's
-- kind that names the entity, if any, the row as hashed, and the raw values
-- it was made from. Without `emit` it builds no fields.
local readers = {}

readers[lanes.NETWORK] = function(api, emit, ids, selected)
	local rows, seen, baseEdges = {}, {}, selected and selected.baseEdges or {}
	local net = { map = 0, get = 0, lanes = 0, junctions = 0, edges = 0, laneConfigs = 0 }
	lanes.cost.net = net
	local t0 = clock()
	local map = selected and { selected.edges } or api.engine.system.streetSystem.getNode2SegmentMap()
	local t1 = clock()
	if t0 and t1 then net.map = t1 - t0 end
	for _, segments in pairs(map) do
		for _, e in pairs(segments) do
			if not seen[e] then
				seen[e] = true
				local g0 = clock()
				local edge = baseEdges[e] or component(api, e, "BASE_EDGE")
				baseEdges[e] = edge or false
				local g1 = clock()
				if g0 and g1 then net.get = net.get + (g1 - g0) end
				net.edges = net.edges + 1
				if edge then
					local l0 = clock()
					local row = lanes.edgeRow(edge, net, api)
					local l1 = clock()
					if l0 and l1 then net.lanes = net.lanes + (l1 - l0) end
					rows[#rows + 1] = row
					if emit then
						emit(nil, e, row, "p0=" .. vecFull(edge.position0) .. " p1=" .. vecFull(edge.position1)
							.. " template=" .. tostring(edge.roadTemplate), { edge.position0, edge.position1 })
					end
				end
			end
		end
	end
	local j0 = clock()
	local junctionRows = junctions.rows(api, baseEdges, selected and selected.nodes)
	local j1 = clock()
	if j0 and j1 then net.junctions = j1 - j0 end
	for _, row in ipairs(junctionRows) do
		rows[#rows+1] = "junction:" .. row
		if emit then emit(nil, nil, "junction:" .. row, "", false) end
	end
	-- Each level crossing's type: of the tile's nodes, or of the world.
	local crossingRows, points = crossings.rows(api, selected and selected.nodes)
	for _, row in ipairs(crossingRows) do
		rows[#rows + 1] = row
		if emit then emit(nil, nil, row, "", points[row]) end
	end
	return summary(rows)
end

readers[lanes.CONSTRUCTIONS] = function(api, emit, ids, selected)
	local rows = {}
	for _, e in ipairs(selected and selected.constructions or entities(api, "CONSTRUCTION")) do
		local c = component(api, e, "CONSTRUCTION")
		if c then
			local t = c.transf
			local x, y = 0, 0
			if t then x, y = t[13] or 0, t[14] or 0 end
			local row = string.format("%s@%s,%s", tostring(c.fileName), q01(x), q01(y))
			rows[#rows + 1] = row
			if emit then
				emit("industries", e, row, "file=" .. tostring(c.fileName) .. " x=" .. full(x) .. " y=" .. full(y)
					.. " z=" .. full(t and t[15]))
			end
		end
	end
	return summary(rows)
end

readers[lanes.LINES] = function(api, emit, ids)
	local rows = {}
	for _, e in pairs(api.engine.system.lineSystem.getLines()) do
		local line = component(api, e, "LINE")
		local row = tostring(line and line.stops and #line.stops or "?")
		rows[#rows + 1] = row
		if emit then
			local fields = { "stops=" .. row }
			local stops = get(line, "stops")
			local n = tonumber(row) or 0
			for i = 1, n do
				local stop = get(stops, i)
				fields[#fields + 1] = "stop" .. i .. "=" .. ids("groups", get(stop, "stationGroup"), "group") .. "/"
					.. full(get(stop, "station")) .. "/" .. full(get(stop, "terminal"))
			end
			emit("lines", e, row, table.concat(fields, " "))
		end
	end
	return summary(rows)
end

-- A vehicle's free capacity by line stop and cargo type
-- (TransportVehicle.lineStop2cargo2available) as `a/b|c/d`, stops apart:
-- what it has room for, so what it carries. For a dump only.
local function freeCapacity(byStop)
	if byStop == nil then return "nil" end
	local ok, text = pcall(function()
		local out = {}
		for i = 1, #byStop do
			local cargo = byStop[i]
			local values = {}
			for j = 1, #cargo do values[#values + 1] = full(cargo[j]) end
			out[#out + 1] = table.concat(values, "/")
		end
		return table.concat(out, "|")
	end)
	if ok then return text end
	return "err"
end

readers[lanes.VEHICLES] = function(api, emit, ids)
	local rows = {}
	-- Geometry is diagnostic only, cached across vehicles and bounded per dump.
	local geometry, geometryReads, routeRecords = {}, 0, 0
	local function edgeGeometry(entity)
		if entity == nil then return "geometry=missing" end
		if geometry[entity] then return geometry[entity] end
		if geometryReads >= 1024 then return "geometry=budget" end
		geometryReads = geometryReads + 1
		local edge = component(api, entity, "BASE_EDGE")
		local text = "geometry=unavailable"
		if edge then
			text = "p0=" .. vecFull(get(edge, "position0")) .. " p1=" .. vecFull(get(edge, "position1"))
				.. " t0=" .. vecFull(get(edge, "tangent0")) .. " t1=" .. vecFull(get(edge, "tangent1"))
		end
		geometry[entity] = text
		return text
	end
	for _, e in ipairs(entities(api, "TRANSPORT_VEHICLE")) do
		local v = component(api, e, "TRANSPORT_VEHICLE")
		local path = component(api, e, "MOVE_PATH")
		local d = path and path.dyn
		local where = "-"
		if d and d.pathPos then
			where = string.format("%d@%.2f v%.2f", d.pathPos.edgeIndex, d.pathPos.pos, d.speed)
		end
		local row = tostring(v and v.state) .. ":" .. tostring(v and v.stopIndex) .. ":" .. where
		rows[#rows + 1] = row
		if emit then
			local pos = d and d.pathPos
			local arrival = get(v, "arrivalStationTerminal")
			local route = get(path, "path")
			local detail = ""
			local routeHash, routeCount = "nil", 0
			if route then
				local edges = get(route, "edges") or {}
				routeCount = #edges
				local entries, nearby, recorded = {}, {}, 0
				local current = get(pos, "edgeIndex") or 0
				local edgeLimit = math.min(#edges, 4096)
				-- Route records are optional diagnostics. Sample the start,
				-- the vehicle's current position, and the end rather than
				-- flooding the 192 KiB/min hook-log upload with a whole prefix.
				local selected = {}
				local function selectRoute(index)
					if index >= 0 and index < edgeLimit then selected[index] = true end
				end
				for index = 0, 1 do selectRoute(index) end
				for index = current - 3, current + 8 do selectRoute(index) end
				for index = edgeLimit - 2, edgeLimit - 1 do selectRoute(index) end
				for i = 1, edgeLimit do
					local edge = edges[i]
					local id = get(edge, "edgeId") or get(edge, 1)
					local direction = get(edge, "dir")
					if direction == nil then direction = get(edge, 2) end
					local text = full(get(id, "entity")) .. "/" .. full(get(id, "index")) .. "/" .. tostring(direction)
					entries[#entries + 1] = text
					-- Geometry helps distinguish local IDs from different routes.
					if selected[i - 1] and recorded < 16 and routeRecords < 128 then
						emit("vehicles", e, "-", "route_index=" .. (i - 1) .. " edge_entity=" .. full(get(id, "entity"))
						.. " lane_index=" .. full(get(id, "index")) .. " direction=" .. tostring(direction)
						.. " " .. edgeGeometry(get(id, "entity")), nil, string.format("/path-%04d", i - 1))
						recorded, routeRecords = recorded + 1, routeRecords + 1
					end
					if i >= current - 1 and i <= current + 8 then nearby[#nearby + 1] = (i - 1) .. ":" .. text end
				end
				routeHash = hashStr(table.concat(entries, ";"))
				detail = " path_count=" .. #edges .. " path_hash=" .. routeHash
					.. " path_sampled=" .. #entries .. " path_near=" .. table.concat(nearby, ",")
					.. " path_omitted=" .. math.max(0, #edges - #entries) .. " path_hash_scope=local_ids"
					.. " route_records=" .. recorded .. " route_omitted=" .. (#edges - recorded)
					.. " path_end=" .. full(get(route, "endOffset")) .. " decision_offset=" .. full(get(route, "terminalDecisionOffset"))
					.. " end_param=" .. full(get(path, "endParam")) .. " end_pos=" .. full(get(path, "endPos"))
					.. " blocked=" .. full(get(path, "blocked")) .. " move_state=" .. full(get(path, "state"))
					.. " accel=" .. full(get(d, "accel")) .. " standing=" .. full(get(d, "timeStanding"))
					.. " until_accel=" .. full(get(d, "timeUntilAccel")) .. " approaching=" .. tostring(get(d, "approachingStation"))
			end
			emit("vehicles", e, row, "state=" .. tostring(v and v.state) .. " stop=" .. tostring(v and v.stopIndex)
				.. " line=" .. ids("lines", get(v, "line"), "line")
				.. " edge=" .. full(pos and pos.edgeIndex) .. " pos=" .. full(pos and pos.pos)
				.. " speed=" .. full(d and d.speed)
				.. " arrival=" .. full(get(arrival, "station")) .. "/" .. full(get(arrival, "terminal"))
					.. " arrival_locked=" .. tostring(get(v, "arrivalStationTerminalLocked"))
				.. " load=" .. full(get(v, "loadState")) .. " pending=" .. full(get(get(v, "unloadPendingIncome"), "amount"))
				.. " free=" .. freeCapacity(get(v, "lineStop2cargo2available")) .. detail)
			-- A compact core for every vehicle survives an oversized verbose
			-- dump in the tailer's newest-byte window. It names the vehicle
			-- and the state needed to locate the first movement difference.
			emit("vehicles", e, row, "state=" .. tostring(v and v.state) .. " stop=" .. tostring(v and v.stopIndex)
				.. " line=" .. ids("lines", get(v, "line"), "line")
				.. " edge=" .. full(pos and pos.edgeIndex) .. " pos=" .. full(pos and pos.pos)
				.. " speed=" .. full(d and d.speed) .. " path_count=" .. routeCount
				.. " path_hash=" .. routeHash, nil, "/core")
		end
	end
	return summary(rows)
end

-- A list of numbers as `a/b/c`, read by index (a table or the game's
-- userdata); nil when it does not read.
local function numbers(v)
	if v == nil then return nil end
	local ok, text = pcall(function()
		local out = {}
		for i = 1, #v do out[#out + 1] = full(v[i]) end
		return table.concat(out, "/")
	end)
	if ok then return text end
	return nil
end

-- The finance window's table for the player (computeFinanceTable, the
-- window's own config: four periods), flattened to `key=v/v/v/v` words:
-- transport income per carrier and kind, investments, other entries,
-- loan, interest and totals. Each value a period's column, so the category
-- an amount was booked under shows. For a dump only; "err" when the engine
-- has no such table.
local function financeTable(api, player)
	local ok, text = pcall(function()
		local finance = api.engine.util.finance
		local config = api.type.ChartConfig.new()
		config.count = 4
		local data = finance.computeFinanceTable(player, config)
		local words = {}
		local function add(key, values)
			words[#words + 1] = key .. "=" .. (numbers(values) or "nil")
		end
		data:foreach_carrier(function(carrier)
			data:foreach_transport(function(kind, values)
				add("transport" .. tostring(carrier) .. "." .. tostring(kind), values)
			end, carrier)
		end)
		data:foreach_investment(function(kind, values) add("investment" .. tostring(kind), values) end)
		data:foreach_other(function(kind, values) add("other" .. tostring(kind), values) end)
		for _, key in ipairs({ "loan", "interest", "loanBorrowing", "loanRepayment", "total", "balance" }) do
			add(key, get(data, key))
		end
		-- The engine's maps list in their own order: sorted, two games'
		-- equal tables read alike.
		table.sort(words)
		return table.concat(words, " ")
	end)
	if ok and type(text) == "string" then return text end
	return "err"
end

-- What each vehicle and each line earned and cost (income and maintenance:
-- calculateBalance(..., true), as the game's vehicle and line windows read
-- it), from the game's start to now. A dump of the economy lane emits one
-- entry per vehicle and line with it, so two games' dumps name the vehicle
-- whose takings split (docs/HOOKS.md, "Lane dumps"). For a dump only.
local function takings(api)
	local ok, finance, now = pcall(function()
		local time = component(api, api.engine.util.getWorld(), "GAME_TIME")
		return api.engine.util.finance, time and time.gameTime
	end)
	if not ok or finance == nil or now == nil then
		return function() return "nil" end
	end
	return function(e)
		local read, value = pcall(function() return finance.calculateBalance({ e }, 0, now, true) end)
		if read then return full(value) end
		return "err"
	end
end

-- A game script's state as the game keeps it, by the script's file; nil
-- where the game has no such script.
local function scriptState(api, name)
	local ok, state = pcall(function()
		local entity = api.engine.system.gameScriptSystem.getEntityForGameScript(name)
		if type(entity) ~= "number" or entity < 0 then return nil end
		local c = api.engine.getComponent(entity, api.type.ComponentType.GAME_SCRIPT)
		return c and c.state
	end)
	if ok and type(state) == "table" then return state end
	return nil
end

-- The mod's own game script, under the names the game has given it.
local MOD_SCRIPTS = { "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs", "tpf3mp_1::/tpf3mp_sim.gs" }
local SUBSIDY_SCRIPT = "::/game_mechanics/subventions/subventions.gs"

-- Each company of the room's roster with its balance, "id=entity:balance",
-- in the roster's order; nil with one company or none, or no roster.
local function companyBalances(api)
	local roster
	for _, name in ipairs(MOD_SCRIPTS) do
		local state = scriptState(api, name)
		if state and type(state.companies) == "table" then roster = state.companies break end
	end
	local list = roster and type(roster.list) == "table" and roster.list or {}
	if #list < 2 then return nil end
	local out = {}
	for _, c in ipairs(list) do
		if type(c) == "table" and not c.gone then
			local account = type(c.entity) == "number" and component(api, c.entity, "ACCOUNT")
			local balance = account and account.balance
			out[#out + 1] = tostring(c.id) .. "=" .. tostring(c.entity) .. ":"
				.. (type(balance) == "number" and string.format("%.0f", balance) or "?")
		end
	end
	return table.concat(out, ",")
end

readers[lanes.ECONOMY] = function(api, emit, ids)
	local player = api.engine.util.getPlayer()
	local account = component(api, player, "ACCOUNT")
	local balance = account and account.balance
	local text = tostring(player) .. ":" .. (balance ~= nil and string.format("%.0f", balance) or "?")
	local okC, balances = pcall(companyBalances, api)
	if okC and balances then text = text .. " companies " .. balances end
	local offers = scriptState(api, SUBSIDY_SCRIPT)
	local rows = offers and subsidies.rows(offers)
	if rows then
		text = text .. " subsidies " .. #rows .. ":" .. hashStr(subsidies.clock(offers) .. "\30" .. table.concat(rows, "\30"))
	end
	if emit and rows then
		emit("subsidies", nil, "subsidies:clock", subsidies.clock(offers))
		for i, row in ipairs(rows) do
			emit("subsidies", nil, "subsidies:" .. i, row)
		end
	end
	if emit then
		-- Dump only: the loan, the player's income as the engine sums it,
		-- the time of its last income and the finance table; then each
		-- vehicle's and line's takings. None of it is hashed.
		local function read(f)
			local found, v = pcall(f)
			return found and full(v) or "nil"
		end
		local finance = get(api.engine.util, "finance")
		emit("player", player, text, "balance=" .. full(balance) .. " loan=" .. full(get(account, "loan"))
			.. " time=" .. read(function() return component(api, api.engine.util.getWorld(), "GAME_TIME").gameTime end)
			.. " income=" .. read(function() return finance.calcIncomeSince(0, player) end)
			.. " last_income=" .. read(function() return finance.getLastIncomeTime(player) end)
			.. " " .. financeTable(api, player))
		local taken = takings(api)
		for _, e in ipairs(entities(api, "TRANSPORT_VEHICLE")) do
			local v = component(api, e, "TRANSPORT_VEHICLE")
			emit("vehicles", e, "takings:" .. tostring(e), "takings=" .. taken(e)
				.. " line=" .. ids("lines", get(v, "line"), "line"))
		end
		local lines = {}
		pcall(function() for _, l in pairs(api.engine.system.lineSystem.getLines()) do lines[#lines + 1] = l end end)
		table.sort(lines)
		for _, l in ipairs(lines) do
			emit("lines", l, "takings:" .. tostring(l), "takings=" .. taken(l))
		end
	end
	return text
end

readers[lanes.TOWNS] = function(api, emit)
	local map = api.engine.system.townBuildingSystem.getTown2BuildingMap()
	local rows = {}
	local growth = emit and townGrowth(api)
	for _, town in ipairs(entities(api, "TOWN")) do
		local count = 0
		local buildings = map and map[town]
		if type(buildings) == "table" then
			for _ in pairs(buildings) do count = count + 1 end
		end
		local row = tostring(town) .. ":" .. count
		rows[#rows + 1] = row
		if emit then
			-- The town's size factors at full precision, and the growth
			-- script's experience and level: what makeTownUpdateSizeCmd is
			-- made from (docs/HOOKS.md, "The town trace").
			local size = get(component(api, town, "TOWN"), "sizeFactors")
			local factors = {}
			for i = 1, 3 do factors[i] = full(get(size, i)) end
			local data = growth(town)
			emit("towns", town, row, "buildings=" .. count .. " size=" .. table.concat(factors, ",")
				.. " experience=" .. full(get(data, "experience")) .. " level=" .. full(get(data, "level")))
		end
	end
	return summary(rows)
end

readers[lanes.PEOPLE] = function(api, emit)
	local text = tostring(#entities(api, "SIM_PERSON"))
	if emit then emit("people", nil, text, "count=" .. text) end
	return text
end

-- Every lane's text, by lane number, and the lanes that could not be read
-- with why, from the `api` of the state the game script runs in.
function lanes.read(api)
	local out, failed = {}, {}
	lanes.cost = { lanes = {}, sort = 0, hash = 0, bytes = 0 }
	for lane = lanes.NETWORK, lanes.PEOPLE do
		local t0 = clock()
		local ok, text = pcall(readers[lane], api)
		local t1 = clock()
		if t0 and t1 then lanes.cost.lanes[lane] = t1 - t0 end
		if ok and type(text) == "string" then
			out[lane] = text
		else
			out[lane] = "err"
			failed[#failed + 1] = lane .. ": " .. tostring(text)
		end
	end
	return out, failed
end
lanes.clock = clock

-- Read just the static world intersecting a box. All engine references have
-- this call's lifetime; nothing borrowed is kept for the next simulation step.
-- Junctions at either end of intersecting edges are included even when their
-- position is outside the box. This also covers long edges and tile borders.
function lanes.spatial(api, box, limit)
	local found, selected = {}, { edges = {}, nodes = {}, baseEdges = {}, constructions = {} }
	api.engine.system.octreeSystem.findIntersectingEntities(box, function(e)
		-- Only collect IDs in the engine's callback. Component access and all
		-- fallible canonicalization happen after the callback has returned.
		found[e] = true
	end)
	for e in pairs(found) do
		local edge = component(api, e, "BASE_EDGE")
		if edge then
			selected.edges[#selected.edges + 1] = e
			selected.baseEdges[e] = edge
			selected.nodes[edge.node0], selected.nodes[edge.node1] = true, true
		end
		if component(api, e, "CONSTRUCTION") then
			selected.constructions[#selected.constructions + 1] = e
		end
	end
	if limit and #selected.edges + #selected.constructions > limit then
		local nodes = 0
		for _ in pairs(selected.nodes) do nodes = nodes + 1 end
		-- The split decision itself is compared. If replicas have different
		-- static inventories, they cannot silently follow different schedules.
		return { [lanes.NETWORK] = "split:" .. #selected.edges .. ":" .. nodes,
			[lanes.CONSTRUCTIONS] = "split:" .. #selected.constructions }, selected, true
	end
	local out = {
		[lanes.NETWORK] = readers[lanes.NETWORK](api, nil, nil, selected),
		[lanes.CONSTRUCTIONS] = readers[lanes.CONSTRUCTIONS](api, nil, nil, selected),
	}
	return out, selected
end

-- Rolling checks in parts (version 2): where the hook reads the static
-- lanes natively (tpf3mp_native.part, crates/tpf3mp-hook/src/netread.rs),
-- every update reads one of PARTS parts of them, so that every PARTS
-- updates the whole map is read. An object's part follows from its row
-- alone: the place the row names it at (an edge by the lower of its two
-- ends, a junction by its node, a construction by its own), in the cell of
-- CELL_TENTHS tenths of a metre (CELL_MM millimetres for a junction row's
-- node key), the cells taking turns in the parts.
lanes.PARTS = 10
local CELL_TENTHS, CELL_MM = 2560, 256000

local function partOf(x, y, side, n)
	return (math.floor(x / side) + 3 * math.floor(y / side)) % n
end
lanes.partOf = partOf

local function tenthsOf(text)
	local v = tonumber(text)
	if type(v) ~= "number" or v ~= v then error("a row's place is no number", 0) end
	return math.floor(v * 10 + 0.5)
end

-- The part of `n` a canonical row of `lane` (NETWORK or CONSTRUCTIONS) is in.
function lanes.rowPart(lane, row, n)
	if lane == lanes.CONSTRUCTIONS then
		local x, y = string.match(row, "@([^@,]+),([^@,]+)$")
		if not x then error("a construction row without its place", 0) end
		return partOf(tenthsOf(x), tenthsOf(y), CELL_TENTHS, n)
	end
	-- A level crossing by the first of its nodes, in millimetres.
	local cx, cy = string.match(row, "^crossing:([^,|;]+),([^,|;]+),")
	if cx then
		local x, y = tonumber(cx), tonumber(cy)
		if type(x) ~= "number" or type(y) ~= "number" then error("a crossing row without its place", 0) end
		return partOf(x, y, CELL_MM, n)
	end
	local jx, jy = string.match(row, "^junction:%a+:([^,|]+),([^,|]+),")
	if jx then
		local x, y = tonumber(jx), tonumber(jy)
		if type(x) ~= "number" or type(y) ~= "number" then error("a junction row without its place", 0) end
		return partOf(x, y, CELL_MM, n)
	end
	-- An edge by the lower of its two ends, by x, then y, then z.
	local ax, ay, az, bx, by, bz = string.match(row, "^([^,>]+),([^,>]+),([^,>]+)>([^,>]+),([^,>]+),([^,>:]+):")
	if not ax then error("an edge row without its place", 0) end
	ax, ay, az, bx, by, bz = tenthsOf(ax), tenthsOf(ay), tenthsOf(az), tenthsOf(bx), tenthsOf(by), tenthsOf(bz)
	if bx < ax or (bx == ax and (by < ay or (by == ay and bz < az))) then ax, ay = bx, by end
	return partOf(ax, ay, CELL_TENTHS, n)
end

-- A part reads one kind of objects in turn (or all of them): its edges,
-- its junctions (both the network lane's), its constructions.
lanes.KINDS = { "edges", "junctions", "constructions" }

-- The kind of objects a canonical row of `lane` is of. A level crossing is
-- read with the junctions.
function lanes.rowKind(lane, row)
	if lane == lanes.CONSTRUCTIONS then return "constructions" end
	if string.sub(row, 1, 9) == "junction:" then return "junctions" end
	if string.sub(row, 1, 9) == "crossing:" then return "junctions" end
	return "edges"
end

-- Whether `kind` (or "all") holds objects of `of`.
function lanes.kindHas(kind, of)
	return kind == "all" or kind == of
end

-- Whether `kind` reads any of `lane`.
function lanes.kindReads(kind, lane)
	if kind == "all" then return true end
	if lane == lanes.CONSTRUCTIONS then return kind == "constructions" end
	return kind == "edges" or kind == "junctions"
end

-- The hook's configured scan plan, even when its mode is off; false when
-- this hook has no native scanner. The mode and partition are saved with
-- either rolling history so a resumed scan cannot silently change plans.
function lanes.nativeParts()
	local native = tpf3mp_native
	if not (type(native) == "table" and type(native.part) == "function"
		and type(native.partTexts) == "function") then return false end
	local plan = native.part()
	if plan == nil then return false end
	return { mode = plan.mode, parts = plan.parts, stride = plan.stride }
end

-- The part's rows as this Lua makes them (the reference the hook's are
-- compared with): every row of the two lanes, kept where it is in part k;
-- read by this Lua alone, without the hook's decoders of a component
-- (laneRows, junctionConfig: lanes.decoders and junctions.decoders off),
-- so that none of the hook's reading is compared with itself; of the same
-- owned copies as ever. The switches are as before however the read ends.
local function luaPart(api, n, k, kind)
	local rows = { [lanes.NETWORK] = {}, [lanes.CONSTRUCTIONS] = {} }
	local before = { lanes.decoders, junctions.decoders }
	lanes.decoders, junctions.decoders = false, false
	local ok, why = pcall(function()
		for _, lane in ipairs({ lanes.NETWORK, lanes.CONSTRUCTIONS }) do
			local list = rows[lane]
			if lanes.kindReads(kind, lane) then
				readers[lane](api, function(_, _, row)
					if lanes.rowKind(lane, row) ~= nil and lanes.kindHas(kind, lanes.rowKind(lane, row))
						and lanes.rowPart(lane, row, n) == k then
						list[#list + 1] = row
					end
				end)
			end
		end
	end)
	lanes.decoders, junctions.decoders = before[1], before[2]
	if not ok then error(why, 0) end
	return rows
end
lanes.partRows = luaPart

-- Whether two sorted lists hold the same rows, as many times each.
local function sameRows(a, b)
	if #a ~= #b then return false end
	for i = 1, #a do
		if a[i] ~= b[i] then return false end
	end
	return true
end

-- Up to three rows in one sorted list and not the other.
local function onlyIn(a, b)
	local out, i, j = {}, 1, 1
	while i <= #a and #out < 3 do
		if j > #b or a[i] < b[j] then out[#out + 1] = a[i] i = i + 1
		elseif a[i] == b[j] then i, j = i + 1, j + 1
		else j = j + 1 end
	end
	return out
end

-- How the parts read and compared since the last checkpoint, for the log only.
local compared = { agree = 0, differ = 0, nativeMs = 0, nativeMax = 0, reads = 0, timing = nil, byKind = {} }

local function countsReset()
	compared.agree, compared.differ, compared.nativeMs, compared.nativeMax, compared.reads = 0, 0, 0, 0, 0
	compared.byKind = {}
end

-- Each kind's native reads since the last checkpoint: "kind n/mean/max".
local function byKindText()
	local out = {}
	for _, kind in ipairs(lanes.KINDS) do
		local c = compared.byKind[kind]
		if c then out[#out + 1] = string.format("%s %d/%.2f/%.2f", kind, c.n, c.sum / c.n, c.max) end
	end
	return table.concat(out, ", ")
end

local function rollingParts(api, scan, step, checkpoint)
	local t0 = clock()
	lanes.cost = { lanes = {}, sort = 0, hash = 0, bytes = 0 }
	local n, stride = scan.parts, scan.stride or 1
	for _, v in ipairs({ n, stride }) do
		if type(v) ~= "number" or v < 1 or v % 1 ~= 0 then error("invalid world-check parts", 0) end
	end
	-- One part every `stride` updates, in turn: of one kind of objects at a
	-- time, the kinds taking turns before the next part.
	local slot = (step - 1) % stride == 0
	local turn = math.floor((step - 1) / stride)
	local kinds = #lanes.KINDS
	local kind = lanes.KINDS[turn % kinds + 1]
	local k = math.floor(turn / kinds) % n
	-- A history of parts goes on only in parts, every update: never another
	-- reading in their place, not even between two parts.
	if not lanes.nativeParts() then error("this game reads no parts of the world", 0) end
	local values, note = {}, nil
	if slot then
		local native = tpf3mp_native
		local read = type(native) == "table" and type(native.part) == "function" and native.part(n, k, kind) or nil
		-- A history of parts goes on only in parts: never another reading in
		-- their place.
		if read == nil then error("this game reads no parts of the world", 0) end
		if read.why then error("part " .. k .. " of the world did not read: " .. tostring(read.why), 0) end
		local compare = read.mode == "compare"
		local preferences, lights = junctions.names(api, read.lights)
		local deferred = junctions.rowsOf(api, read.deferred)
		-- The hook reads the part's level crossings; this Lua names their types.
		-- A hook that hands none read none: never a part without its crossings.
		if type(read.crossingTypes) ~= "table" then
			error("part " .. k .. " of the world read no level crossings", 0)
		end
		local crossingTypes = crossings.names(api, read.crossingTypes)
		local net, cons, netRows, consRows = native.partTexts(n, k, kind, preferences, lights, deferred, compare,
			crossingTypes)
		if net == nil and cons == nil then
			error("part " .. k .. " of the world has no texts: " .. tostring(netRows), 0)
		end
		if (net ~= nil) ~= lanes.kindReads(kind, lanes.NETWORK)
			or (cons ~= nil) ~= lanes.kindReads(kind, lanes.CONSTRUCTIONS) then
			error("part " .. k .. " of the world has texts of other lanes than its " .. kind, 0)
		end
		compared.reads = compared.reads + 1
		compared.nativeMs = compared.nativeMs + (read.ms or 0)
		compared.nativeMax = math.max(compared.nativeMax, read.ms or 0)
		compared.timing = read.timing
		local c = compared.byKind[kind] or { n = 0, sum = 0, max = 0 }
		c.n, c.sum, c.max = c.n + 1, c.sum + (read.ms or 0), math.max(c.max, read.ms or 0)
		compared.byKind[kind] = c
		values[lanes.NETWORK], values[lanes.CONSTRUCTIONS] = net, cons
		if compare then
			local own = luaPart(api, n, k, kind)
			local theirs = { [lanes.NETWORK] = netRows, [lanes.CONSTRUCTIONS] = consRows }
			local differ = {}
			for _, lane in ipairs({ lanes.NETWORK, lanes.CONSTRUCTIONS }) do
				if lanes.kindReads(kind, lane) then
					local text = summary(own[lane])
					-- The rows themselves, not only their count and hash.
					-- The network's text adds its crossings after its own:
					-- compared, the rows count.
					local native = values[lane]
					if lane == lanes.NETWORK then native = summary(theirs[lane] or {}) end
					if text ~= native or not sameRows(own[lane], theirs[lane] or {}) then
						differ[#differ + 1] = string.format("lane %d lua %s native %s; only lua: %s; only native: %s",
							lane, text, tostring(values[lane]), table.concat(onlyIn(own[lane], theirs[lane] or {}), " || "),
							table.concat(onlyIn(theirs[lane] or {}, own[lane]), " || "))
					end
					-- Compared, this Lua's own text counts.
					values[lane] = text
				end
			end
			if #differ > 0 then
				compared.differ = compared.differ + 1
				note = string.format("part %d/%d (%s) at step %d differs: %s", k, n, kind, step, table.concat(differ, "; "))
			else
				compared.agree = compared.agree + 1
			end
		end
	end
	local dynamic = lanes.LINES + (step - 1) % 5
	values[dynamic] = readers[dynamic](api)
	local context = slot and string.format("%d|parts|%s|%d/%d|", step, kind, k, n) or string.format("%d|", step)
	for lane, text in pairs(values) do
		if type(text) ~= "string" or text == "err" then error("world-check lane " .. lane .. " was not read", 0) end
		scan.hashes[lane] = fastHash((scan.hashes[lane] or "rolling-v2") .. context .. text)
		scan.counts[lane] = (scan.counts[lane] or 0) + 1
	end
	scan.step = step
	local t1 = clock()
	local ms = t0 and t1 and (t1 - t0) * 1000 or 0
	local out, report
	if slot and k == n - 1 and turn % kinds == kinds - 1 then
		scan.sweeps = scan.sweeps + 1
		report = string.format("rolling world sweep: cycle=%d steps=%d-%d samples=%d",
			scan.sweeps, scan.sweepStart, step, step - scan.sweepStart + 1)
		scan.sweepStart = step + 1
	end
	if checkpoint then
		out = {}
		local texts = {}
		for lane = lanes.NETWORK, lanes.PEOPLE do
			out[lane] = string.format("rolling-v2:%d-%d:%d:%s", scan.first, step,
				scan.counts[lane] or 0, scan.hashes[lane] or "empty")
			texts[#texts + 1] = out[lane]
		end
		local line = string.format("rolling world check: steps=%d-%d part=%d/%d last_ms=%.3f"
			.. " native_mean_ms=%.2f native_max_ms=%.2f by kind [%s] (%s)%s signature=%s",
			scan.first, step, k, n, ms, compared.nativeMs / math.max(1, compared.reads), compared.nativeMax,
			byKindText(), tostring(compared.timing),
			(compared.agree + compared.differ > 0)
				and string.format(" compared agree=%d differ=%d", compared.agree, compared.differ) or "",
			fastHash(table.concat(texts, "\n")))
		report = report and (report .. "; " .. line) or line
		countsReset()
		scan.first, scan.hashes, scan.counts = step + 1, {}, {}
	end
	if note then report = report and (report .. "; " .. note) or note end
	return scan, out, report, ms
end

-- A rolling observation window, saved with the world so a joining/rebased
-- game resumes the exact same window. Only numbers and digest strings survive
-- an update. Samples use absolute room steps, never frame rate or a stopwatch.
-- Busy cells subdivide before canonicalizing their components; empty countryside
-- advances without spending hundreds of steps there. The split counts are part
-- of the observations, so a divergent world cannot choose a different schedule
-- unnoticed. Dynamic lanes are read once every five updates.
function lanes.rolling(api, previous, step, checkpoint)
	if type(step) ~= "number" or step < 1 or step % 1 ~= 0 then error("invalid world-check step", 0) end
	local plan = lanes.nativeParts()
	local scan = previous
	if step == 1 then
		if plan and plan.mode ~= "off" then
			scan = { version = 2, mode = plan.mode, step = 0, first = 1, hashes = {}, counts = {},
				parts = plan.parts or lanes.PARTS, stride = plan.stride or 1, sweepStart = 1, sweeps = 0 }
		else
			scan = { version = 1, step = 0, first = 1, hashes = {}, counts = {},
				tile = 0, pending = {}, sweepStart = 1, sweeps = 0,
				mode = plan and plan.mode or nil, parts = plan and plan.parts or nil,
				stride = plan and plan.stride or nil }
		end
	end
	if type(scan) ~= "table" or (scan.version ~= 1 and scan.version ~= 2) or scan.step ~= step - 1 then
		error("the rolling world-check history is missing or skipped an update", 0)
	end
	if scan.version == 2 then
		if not plan or plan.mode == "off" then
			error("saved rolling-v2 history has no native part plan or it is disabled", 0)
		end
		if scan.mode ~= plan.mode or scan.parts ~= (plan.parts or lanes.PARTS)
			or scan.stride ~= (plan.stride or 1) then
			error("saved rolling-v2 plan does not match this game's native part plan", 0)
		end
		return rollingParts(api, scan, step, checkpoint)
	end
	if plan then
		if plan.mode ~= "off" then
			error("saved rolling-v1 history does not match this game's native scan plan", 0)
		end
		if scan.mode == nil and scan.parts == nil and scan.stride == nil then
			-- Older v1 saves predate the local plan fields. The off-mode tile
			-- reader is unchanged, so adopt only this all-absent legacy shape.
			scan.mode, scan.parts, scan.stride = plan.mode, plan.parts, plan.stride
		elseif scan.mode ~= plan.mode or scan.parts ~= plan.parts or scan.stride ~= plan.stride then
			error("saved rolling-v1 history does not match this game's native scan plan", 0)
		end
	elseif scan.mode ~= nil or scan.parts ~= nil or scan.stride ~= nil then
		error("saved rolling-v1 history has no native scan plan", 0)
	end
	local t0 = clock()
	lanes.cost = { lanes = {}, sort = 0, hash = 0, bytes = 0 }
	local bounds = api.engine.terrain.getBoundingBox()
	local x0, y0, x1, y1 = bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y
	for _, n in ipairs({x0, y0, x1, y1}) do
		if type(n) ~= "number" or n ~= n or math.abs(n) >= 1000000 then error("invalid world-check bounds", 0) end
	end
	if x1 <= x0 or y1 <= y0 then error("empty world-check bounds", 0) end
	local side = 1024
	local nx, ny = math.ceil((x1 - x0) / side), math.ceil((y1 - y0) / side)
	local total = nx * ny
	local region = table.remove(scan.pending)
	if not region then
		local x, y = scan.tile % nx, math.floor(scan.tile / nx)
		region = { x0 + side * x, y0 + side * y, math.min(x1, x0 + side * (x + 1)), math.min(y1, y0 + side * (y + 1)) }
		scan.tile = (scan.tile + 1) % total
	end
	local minX = region[1] == x0 and -1000000 or region[1]
	local minY = region[2] == y0 and -1000000 or region[2]
	local maxX = region[3] == x1 and 1000000 or region[3]
	local maxY = region[4] == y1 and 1000000 or region[4]
	local box = api.type.Box3.new(api.type.Vec3f.new(minX, minY, -1000000),
		api.type.Vec3f.new(maxX, maxY, 1000000))
	local canSplit = region[3] - region[1] > 32 and region[4] - region[2] > 32
	local values, _, split = lanes.spatial(api, box, canSplit and 32 or nil)
	if split then
		local mx, my = (region[1] + region[3]) / 2, (region[2] + region[4]) / 2
		for _, child in ipairs({ { mx, my, region[3], region[4] }, { region[1], my, mx, region[4] },
			{ mx, region[2], region[3], my }, { region[1], region[2], mx, my } }) do
			scan.pending[#scan.pending + 1] = child
		end
	end
	local dynamic = lanes.LINES + (step - 1) % 5
	values[dynamic] = readers[dynamic](api)
	local context = string.format("%d|%.3f,%.3f,%.3f,%.3f|%.3f,%.3f,%.3f,%.3f|", step,
		x0, y0, x1, y1, region[1], region[2], region[3], region[4])
	for lane, text in pairs(values) do
		if type(text) ~= "string" or text == "err" then error("world-check lane " .. lane .. " was not read", 0) end
		scan.hashes[lane] = fastHash((scan.hashes[lane] or "rolling-v1") .. context .. text)
		scan.counts[lane] = (scan.counts[lane] or 0) + 1
	end
	scan.step = step
	local t1 = clock()
	local ms = t0 and t1 and (t1 - t0) * 1000 or 0
	local out, report
	if scan.tile == 0 and #scan.pending == 0 then
		scan.sweeps = scan.sweeps + 1
		report = string.format("rolling world sweep: cycle=%d steps=%d-%d samples=%d",
			scan.sweeps, scan.sweepStart, step, step - scan.sweepStart + 1)
		scan.sweepStart = step + 1
	end
	if checkpoint then
		out = {}
		local texts = {}
		for lane = lanes.NETWORK, lanes.PEOPLE do
			out[lane] = string.format("rolling-v1:%d-%d:%d:%s", scan.first, step,
				scan.counts[lane] or 0, scan.hashes[lane] or "empty")
			texts[#texts + 1] = out[lane]
		end
		local checkpointReport = string.format("rolling world check: steps=%d-%d tile=%d/%d pending=%d last_ms=%.3f signature=%s", scan.first, step, scan.tile, total, #scan.pending, ms, fastHash(table.concat(texts, "\n")))
		report = report and (report .. "; " .. checkpointReport) or checkpointReport
		scan.first, scan.hashes, scan.counts = step + 1, {}, {}
	end
	return scan, out, report, ms
end

-- The last read's cost as one line for the log, in milliseconds.
function lanes.costLine()
	local c, parts = lanes.cost, {}
	local names = { [0] = "network", "constructions", "lines", "vehicles", "economy", "towns", "people" }
	local total = 0
	for lane = lanes.NETWORK, lanes.PEOPLE do
		local t = c.lanes[lane]
		if t then total = total + t end
		parts[#parts + 1] = names[lane] .. " " .. (t and string.format("%.1f", t * 1000) or "?")
	end
	local n = c.net or {}
	return string.format("lanes read in %.1f ms: %s; of it sort+concat %.1f ms, hash %.1f ms over %d bytes;"
		.. " network: map %.1f ms, %d edges' getComponent %.1f ms, their %d lane configs %.1f ms, junctions %.1f ms",
		total * 1000, table.concat(parts, ", "), c.sort * 1000, c.hash * 1000, c.bytes,
		(n.map or 0) * 1000, n.edges or 0, (n.get or 0) * 1000, n.laneConfigs or 0, (n.lanes or 0) * 1000,
		(n.junctions or 0) * 1000)
end

-- The registry's kinds as a dump names their ids.
local PREFIX = { vehicles = "vehicle", lines = "line", towns = "town", industries = "industry",
	groups = "group" }

-- Lane `lane` entry by entry, as a list of lines, sorted: each
-- `<key> <field=value> ... entity=<e> row=<row>`, then `summary <text>`, the
-- lane's text as lanes.read reads it; a lane that cannot be read is the one
-- line `err <why>`. `reg` is the registry of the game script's state
-- (tpf3mp/registry.lua), which names the keys; nil names none.
-- Whether one of `points` (vectors) lies inside `box`, { x0, y0, x1, y1 }
-- with x0 <= x1 and y0 <= y1.
local function inBox(box, points)
	for _, p in ipairs(points) do
		local x, y = get(p, "x"), get(p, "y")
		if type(x) ~= "number" and type(p) == "table" then x, y = p[1], p[2] end
		if type(x) == "number" and type(y) == "number"
			and x >= box[1] and x <= box[3] and y >= box[2] and y <= box[4] then
			return true
		end
	end
	return false
end

-- With `box` ({ x0, y0, x1, y1 }, the network lane only), the entries are
-- the edges with an end inside it; the summary stays the whole lane's.
function lanes.dump(api, lane, reg, box)
	local reader = readers[lane]
	if lane ~= lanes.NETWORK or type(box) ~= "table" or #box ~= 4 then box = nil end
	if reader == nil then return { "err no lane " .. tostring(lane) } end
	-- The registry's ids by entity, per kind, made when first asked.
	local byEntity = {}
	local function idOf(kind, e)
		if kind == nil or e == nil then return nil end
		local map = byEntity[kind]
		if map == nil then
			map = {}
			local r = type(reg) == "table" and reg[kind]
			for _, pair in ipairs(type(r) == "table" and r.bound or {}) do map[pair[2]] = pair[1] end
			byEntity[kind] = map
		end
		return map[e]
	end
	-- An entity a field names, by its id if it has one.
	local function ids(kind, e, prefix)
		local id = idOf(kind, e)
		if id ~= nil then return prefix .. "-" .. id end
		if e == nil then return "nil" end
		return "entity-" .. tostring(e)
	end
	local entries = {}
	local function emit(kind, e, row, fields, points, suffix)
		if box and not (type(points) == "table" and inBox(box, points)) then return end
		local id = idOf(kind, e)
		local key, order
		if id ~= nil then
			key = PREFIX[kind] .. "-" .. id
			order = string.format("0 %s %015d", PREFIX[kind], id)
		elseif kind == "player" or kind == "people" then
			key, order = kind, "0 " .. kind
		else
			key = "row:" .. string.gsub(row, "%s", "_")
			order = "1 " .. row .. "\30" .. string.format("%015d", tonumber(e) or 0)
		end
		key = key .. (suffix or "")
		-- Core first, then verbose base, then optional routes at the hook's
		-- line limit. The hook writes these in the opposite priority order.
		local priority = suffix == "/core" and "0 " or (suffix and "2 " or "1 ")
		order = priority .. order .. (suffix or "")
		-- Only the diagnostic spelling changes: '@' in numeric vehicle rows
		-- looks like an email to the relay's privacy filter. Hash input is untouched.
		if kind == "vehicles" then row = string.gsub(row, "@", "~") end
		local text = key .. " " .. fields
		if e ~= nil then text = text .. " entity=" .. tostring(e) end
		text = text .. " row=" .. row
		if kind == "vehicles" and #text > 850 then
			-- Keep below DiagnosticText's 1024 bytes including timestamp and
			-- lane/step prefix. Each piece has its own stable comparison key.
			local chunks, chunk = {}, ""
			for word in (fields .. " entity=" .. tostring(e) .. " row=" .. row):gmatch("%S+") do
				local name, value = word:match("^([^=]+)=(.*)$")
				local words = { word }
				if #word > 650 and name then
					words = {}
					for at = 1, #value, 600 do
						words[#words + 1] = name .. "_part" .. #words .. "=" .. value:sub(at, at + 599)
					end
				end
				for _, part in ipairs(words) do
					if #chunk + #part > 700 then
						chunks[#chunks + 1] = chunk
						chunk = ""
					end
					chunk = chunk == "" and part or chunk .. " " .. part
				end
			end
			if chunk ~= "" then chunks[#chunks + 1] = chunk end
			for i, part in ipairs(chunks) do
				local tail = i == 1 and "" or string.format("/detail-%03d", i)
				entries[#entries + 1] = { order = order .. tail, text = key .. tail .. " " .. part }
			end
		else
			entries[#entries + 1] = { order = order, text = text }
		end
	end
	local ok, text = pcall(reader, api, emit, ids)
	if not ok or type(text) ~= "string" then return { "err " .. tostring(text) } end
	table.sort(entries, function(a, b) return a.order < b.order end)
	local out = {}
	for i, entry in ipairs(entries) do out[i] = entry.text end
	out[#out + 1] = "summary " .. text
	return out
end

lanes.hash = hashStr

-- One entity as the edge watch reads it, a line of text: `edge node0=
-- node1= p0= p1= t0= t1= n0= n1= type=` (the ends and tangents of the
-- edge, then its nodes' positions), `node pos=`, or `absent`. Never raises.
function lanes.watch(api, entity)
	local ok, text = pcall(function()
		local edge = component(api, entity, "BASE_EDGE")
		if edge then
			local n0, n1 = get(edge, "node0"), get(edge, "node1")
			local function nodePos(n)
				if n == nil then return "nil" end
				return vecFull(get(component(api, n, "BASE_NODE"), "position"))
			end
			return "edge node0=" .. full(n0) .. " node1=" .. full(n1)
				.. " p0=" .. vecFull(get(edge, "position0")) .. " p1=" .. vecFull(get(edge, "position1"))
				.. " t0=" .. vecFull(get(edge, "tangent0")) .. " t1=" .. vecFull(get(edge, "tangent1"))
				.. " n0=" .. nodePos(n0) .. " n1=" .. nodePos(n1)
				.. " type=" .. full(get(edge, "type")) .. " template=" .. full(get(edge, "roadTemplate"))
		end
		local node = component(api, entity, "BASE_NODE")
		if node then return "node pos=" .. vecFull(get(node, "position")) end
		return "absent"
	end)
	if ok then return text end
	return "err " .. tostring(text)
end

return lanes
