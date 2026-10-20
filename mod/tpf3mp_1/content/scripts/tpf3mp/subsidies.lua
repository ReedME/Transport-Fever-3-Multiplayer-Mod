-- tpf3mp/subsidies.lua -- a subsidy is its taker's (docs/HOOKS.md,
-- "Subsidies").
--
-- The game's subsidy script (::/game_mechanics/subventions/subventions.gs,
-- build 40408's subventions.script.tl) runs each kind of subsidy through
-- that kind's own script, which its resource names (`scriptFile`, as
-- "<file>@<name>"; every call goes through util.useFn(scriptFile ..
-- ".<function>"), looked up again at each call). Each kind counts progress
-- in its `handleEvent`, from the simulation's events, for anyone's
-- transport:
--
-- - deliver_passengers: `OnStartedLineUsage` (SimPersonSystem), a person
--   starting a direct line between the two towns completes it; then
--   `OnCalcTicketPrice` doubles that line's tickets;
-- - deliver_cargo and deliver_cargo_town: `OnCalcTicketPrice`
--   (TransportVehicleSystem), each cargo delivered to the industry, or to
--   the town's buildings, counts one, by the line that carried it; once
--   completed, those lines' cargo pays double;
-- - deliver_workers: completes when the industry's workers are boosted
--   (industry_util.isPersonCapacityBoosted), a world state the engine keeps
--   for no company. Not filtered: no source says whose transport boosted it.
--
-- In a room each kind's script is reached through this module instead
-- (`redirect`, the mod's run script's modifier on the subsidy resources,
-- and tpf3mp_sim/subsidies.script.lua, which wraps each kind's own table),
-- in every Lua state the same, since the resource is the whole game's:
--
-- - the room's accept sends the subsidy script's own `onAccept` with the
--   taking company's player entity (`tpf3mpCompany`, tpf3mp/apply.lua). The
--   subsidy script hands the event on to the accepted subsidy's
--   `handleEvent` at once (subventions.script.tl, handleSubventionEvent),
--   where the wrapper keeps it in the subsidy's own data (`tpf3mpTaker`):
--   the script's state, the same in every game;
-- - from then on, the wrapper hands the kind's `handleEvent` only the
--   events' entries whose line the taker owns (PLAYER_OWNED), and maps the
--   ticket multipliers it answers back to the event's own entries.
--
-- A subsidy with no taker kept (single player, a save from before, a game
-- without the room's accept) runs as the game has it.
--
-- `summary` reads the subsidy script's state in one line per subsidy, for
-- the economy lane (tpf3mp/lanes.lua) and the hook's log: two games whose
-- offers differ say so at the next checkpoint.
--
-- Pure Lua over the `api` it is given; the tests hand it a fake.

local subsidies = {}

-- The mod's wrapper script, as a subsidy resource names its script.
subsidies.FILTER = "tpf3mp_1::/tpf3mp_sim/subsidies.script"

-- The base game's kinds (build 40408), by the name their script file
-- returns them under: the reference the resource names, and the module.
subsidies.KINDS = {
	deliver_cargo = {
		ref = "::/game_mechanics/subventions/deliver_cargo/deliver_cargo.script@deliver_cargo",
		module = "::/game_mechanics/subventions/deliver_cargo/deliver_cargo.script.tl",
	},
	deliver_cargo_town = {
		ref = "::/game_mechanics/subventions/deliver_cargo_town/deliver_cargo_town.script@deliver_cargo_town",
		module = "::/game_mechanics/subventions/deliver_cargo_town/deliver_cargo_town.script.tl",
	},
	deliver_passengers = {
		ref = "::/game_mechanics/subventions/deliver_passengers/deliver_passengers.script@deliver_passengers",
		module = "::/game_mechanics/subventions/deliver_passengers/deliver_passengers.script.tl",
	},
	deliver_workers = {
		ref = "::/game_mechanics/subventions/deliver_workers/deliver_workers.script@deliver_workers",
		module = "::/game_mechanics/subventions/deliver_workers/deliver_workers.script.tl",
	},
}

-- The kinds in a fixed order.
subsidies.ORDER = { "deliver_cargo", "deliver_cargo_town", "deliver_passengers", "deliver_workers" }

-- The resource modifier (`addModifier("loadGameRes", ...)`): a subsidy
-- resource of a base kind names the mod's wrapper instead; anything else
-- is left as it is. Returns `data`.
function subsidies.redirect(_fileName, data)
	if type(data) ~= "table" or data.type ~= "subvention" or type(data.data) ~= "table" then return data end
	local ref = data.data.scriptFile
	if type(ref) ~= "string" then return data end
	for _, name in ipairs(subsidies.ORDER) do
		if ref == subsidies.KINDS[name].ref then
			data.data.scriptFile = subsidies.FILTER .. "@" .. name
			return data
		end
	end
	return data
end

-- A field of a game value, or nil: the game's userdata raise on a field
-- they lack.
local function get(value, key)
	if value == nil then return nil end
	local ok, v = pcall(function() return value[key] end)
	if ok then return v end
	return nil
end

-- The owner of `entity` (its PLAYER_OWNED player), or nil.
local function ownerOf(api, entity)
	if type(entity) ~= "number" or entity < 0 then return nil end
	local ok, owner = pcall(function()
		local c = api.engine.getComponent(entity, api.type.ComponentType.PLAYER_OWNED)
		return c and c.player
	end)
	if ok and type(owner) == "number" and owner >= 0 then return owner end
	return nil
end

-- The line a person or cargo is on, as deliver_passengers reads it: at a
-- vehicle, else at a terminal.
local function lineOf(api, simEntity)
	local ok, line = pcall(function()
		local v = api.engine.getComponent(simEntity, api.type.ComponentType.SIM_ENTITY_AT_VEHICLE)
		if v then return v.line end
		local t = api.engine.getComponent(simEntity, api.type.ComponentType.SIM_ENTITY_AT_TERMINAL)
		return t and t.line
	end)
	if ok then return line end
	return nil
end

-- The player entity that took subsidy `s`, as the wrapper kept it; nil
-- when none was.
function subsidies.taker(s)
	local taker = get(get(s, "data"), "tpf3mpTaker")
	if type(taker) == "number" then return taker end
	return nil
end

-- `OnStartedLineUsage`'s entries ({ simEntity, ... }) whose line `taker`
-- owns, as a new event parameter.
function subsidies.lineUsageOf(api, param, taker)
	local kept = {}
	local entries = get(param, "entities")
	if entries ~= nil then
		for i = 1, #entries do
			local entry = entries[i]
			local line = get(entry, 2)
			if type(line) ~= "number" then line = lineOf(api, get(entry, 1)) end
			if ownerOf(api, line) == taker then kept[#kept + 1] = entry end
		end
	end
	return { entities = kept }
end

-- `OnCalcTicketPrice`'s entries whose line (`lineEntity`) `taker` owns, as
-- a list, and for each the index of the event's own entry it was.
function subsidies.ticketsOf(api, param, taker)
	local kept, from = {}, {}
	if param ~= nil then
		for i = 1, #param do
			local p = param[i]
			if ownerOf(api, get(p, "lineEntity")) == taker then
				kept[#kept + 1] = p
				from[#kept] = i
			end
		end
	end
	return kept, from
end

-- A kind's answer to a ticket price event (multipliers by entry index),
-- by the event's own indices.
local function remap(answer, from)
	if type(answer) ~= "table" then return answer end
	local out = {}
	for i, multiplier in pairs(answer) do
		local original = from[i]
		if original ~= nil then out[original] = multiplier end
	end
	return out
end

-- Kind `kind`'s table, wrapped: every function its own, but `handleEvent`,
-- which keeps the taker on the room's accept and then hands on only the
-- taker's transport. `api0` is the api to read the world with; nil reads
-- the state's own `api` at each call.
function subsidies.wrap(kind, api0)
	local own = kind.handleEvent
	local wrapped = {}
	wrapped.handleEvent = function(src, id, name, param, s)
		local api = api0 or api
		if id == "Subvention" and name == "onAccept" then
			local company = get(param, "tpf3mpCompany")
			local data = get(s, "data")
			if type(company) == "number" and get(param, "uid") == get(s, "uid") and type(data) == "table" then
				data.tpf3mpTaker = company
			end
		end
		if own == nil then return nil end
		local taker = subsidies.taker(s)
		if taker == nil then return own(src, id, name, param, s) end
		if name == "OnStartedLineUsage" then
			return own(src, id, name, subsidies.lineUsageOf(api, param, taker), s)
		elseif name == "OnCalcTicketPrice" then
			local kept, from = subsidies.ticketsOf(api, param, taker)
			return remap(own(src, id, name, kept, s), from)
		end
		return own(src, id, name, param, s)
	end
	return setmetatable(wrapped, { __index = kind })
end

-- Every base kind, loaded with `require_` (ug_require) and wrapped, by the
-- name the resource reaches it under; a kind that does not load is left
-- out, and its subsidies fail as a missing script fails in the game.
function subsidies.wrapAll(require_, api)
	local out = {}
	for _, name in ipairs(subsidies.ORDER) do
		local ok, module = pcall(require_, subsidies.KINDS[name].module)
		local kind = ok and type(module) == "table" and module[name]
		if type(kind) == "table" then out[name] = subsidies.wrap(kind, api) end
	end
	return out
end

-- The subsidy script's lists, and what each says of a subsidy in it.
local LISTS = {
	{ "proposedSubventions", "offered" },
	{ "activeSubventions", "taken" },
	{ "completedSubventions", "completed" },
	{ "failedSubventions", "failed" },
}

-- A number as text: whole numbers as integers, else at full precision.
-- Never `%d`: the game's Lua 5.2 formats it through a 32-bit C `long` on
-- Windows and raises on anything past 2^31, which offer numbers reach.
local function num(v)
	if type(v) ~= "number" then return "-" end
	if v == 0 then return "0" end
	if v == math.floor(v) and v > -1e15 and v < 1e15 then return string.format("%.0f", v) end
	return string.format("%.17g", v)
end

-- The money a bonus or penalty list books: the sum of its Money amounts.
local function money(list)
	local sum = 0
	for _, b in ipairs(type(list) == "table" and list or {}) do
		local amount = type(b) == "table" and b.type == "Money" and type(b.params) == "table" and b.params.amount
		if type(amount) == "number" then sum = sum + math.floor(amount) end
	end
	return sum
end

-- The subsidy script's `state`, one row per subsidy, in the script's own
-- order: where it is, its number, kind, when it was offered, accepted and
-- completed, its money up front, for completing and for failing, how much
-- it asks and how much is delivered, and when the offer lapses. Nil when
-- there is no state.
function subsidies.rows(state)
	if type(state) ~= "table" then return nil end
	local rows = {}
	for _, list in ipairs(LISTS) do
		for _, s in ipairs(type(state[list[1]]) == "table" and state[list[1]] or {}) do
			if type(s) == "table" then
				local d = type(s.data) == "table" and s.data or {}
				rows[#rows + 1] = table.concat({ list[2], num(s.uid), tostring(s.id), "spawn=" .. num(s.spawnTime),
					"accepted=" .. num(s.acceptedTime), "completed=" .. num(s.completedTime),
					"upfront=" .. num(money(d.upfront)), "complete=" .. num(money(d.complete)),
					"failure=" .. num(money(d.failure)), "deliver=" .. num(d.toDeliver),
					"delivered=" .. num(d.delivered), "lapses=" .. num(d.expireDurationProposed),
					"taker=" .. num(d.tpf3mpTaker) }, " ")
			end
		end
	end
	return rows
end

-- The script's spawn bookkeeping, in one row: the last offer's time and
-- the interval modifier.
function subsidies.clock(state)
	if type(state) ~= "table" then return nil end
	return "last=" .. num(state.lastSpawnTime) .. " modifier=" .. num(state.spawnIntervalModifier)
		.. " pause=" .. tostring(state.pause == true)
end

-- The subsidy script's `state` in one line: its clock, then every row;
-- nil when there is no state.
function subsidies.summary(state)
	local rows = subsidies.rows(state)
	if rows == nil then return nil end
	local out = { subsidies.clock(state) }
	for _, r in ipairs(rows) do out[#out + 1] = r end
	return table.concat(out, "; ")
end

return subsidies
