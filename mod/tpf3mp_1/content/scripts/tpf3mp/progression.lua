-- tpf3mp/progression.lua -- how the room's companies rank up when there is
-- more than one (docs/DECISIONS.md, D23 proposed).
--
-- Transport Fever 3's own rule (build 40408, game_mechanics/company/,
-- investigation/TPF3_PROGRESSION_2026-09-30.md): the company growth script
-- keeps one company, the save's player. Its experience is the highest world
-- population it has seen (the residents of every town, served or not), and
-- its rank the game's thresholds on that, from the population the game began
-- with. With one company in the room nothing here changes that: the game
-- keeps its own score, and a rank the company window takes goes to the
-- growth script as its own event.
--
-- With more than one company each has a score of its own, the sum over the
-- towns of
--
--   population x share x rating / 100
--
-- - population: the town's residents, as the game counts them for its own
--   score (townBuildingSystem.getTown2personCapacitiesMap);
-- - share: the company's share of what was carried for the town: of the
--   cargo delivered to it in the last half year (the game's delivery
--   statistics per line, the window its own delivery rating uses) and of the
--   passengers travelling to and from it on lines (the game's statistics per
--   line, averaged over the same half year here), the two shares weighed by
--   WEIGHTS. A kind nobody carries there does not count;
-- - rating: the company's rating in that town, 0 to 100: the game's town
--   rating (the lowest of its parts, as the towns script computes it) with
--   the two parts a company earns itself taken from its own lines: the
--   happiness of its passengers and how many of its cargo came on time, by
--   the game's own formulas. The other parts (reputation, traffic, noise,
--   pollution) are the town's, the same for every company.
--
-- The score is recomputed four times a game month, at the same game time in
-- every game, from the simulation's state only. As the game's, experience
-- never falls: it is the highest score reached. The rank it reaches is the
-- game's own thresholds (company_progression_util.getLevelAndFraction); the
-- rank taken is the company's, through ApplyRank. The mod's game script
-- keeps it all in its state, which the game saves with the world:
--
--   progression = {
--     quarter = n,                -- the last quarter of a month sampled
--     weights = { cargo =, passengers = },
--     records = { { company = id, entity =, experience =, potential =, level = }, ... },
--     passengers = { { company = id, town = entity, value = }, ... },
--   }
--
-- Pure Lua against the game's `api` and the game's own modules (`game`
-- below); the tests give it fakes.

local progression = {}

-- How a town's share is weighed between its cargo and its passengers.
progression.WEIGHTS = { cargo = 1, passengers = 1 }
-- Samples a game month.
progression.PER_MONTH = 4
-- The passengers' average reaches back this many samples: half a year, the
-- window of the game's delivery statistics.
progression.WINDOW = 24
-- The lines a town's passenger statistics list, at most.
progression.LINES = 1000
-- The parts of a town's rating that are the town's, not a company's
-- (town_util.GetRatings); people_happiness and cargo_delivery are each
-- company's own.
progression.SHARED = { "urban_care", "traffic_congestion", "noise", "pollution" }

-- The game's modules, by the names its own scripts ug_require them.
progression.PATHS = {
	progression = "/game_mechanics/company/company_progression_util.tl",
	towns = "/game_mechanics/towns/town_util.tl",
	company = "/game_mechanics/company/company_util.tl",
}

local function module(name)
	local loaded = package and package.loaded and package.loaded["tpf3mp." .. name]
	if loaded then return loaded end
	if ug_require then return ug_require("tpf3mp_1::/scripts/tpf3mp/" .. name .. ".lua") end
	return require("tpf3mp." .. name)
end
local companies = module("companies")

local function clamp(x, lo, hi)
	if x < lo then return lo end
	if x > hi then return hi end
	return x
end

-- Whether the room has more than one company: then this rule, else the
-- game's own.
function progression.multi(roster)
	return type(roster) == "table" and type(roster.list) == "table" and #companies.live(roster) > 1
end

function progression.ensure(prog)
	if type(prog) ~= "table" then prog = {} end
	if type(prog.records) ~= "table" then prog.records = {} end
	if type(prog.passengers) ~= "table" then prog.passengers = {} end
	if type(prog.weights) ~= "table" then
		prog.weights = { cargo = progression.WEIGHTS.cargo, passengers = progression.WEIGHTS.passengers }
	end
	return prog
end

-- The quarter of a game month now, counted from the game's start; nil where
-- the game does not say.
function progression.quarterNow(api)
	local ok, quarter = pcall(function()
		local gt = api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME)
		local length = api.util.getDefaultMonthDuration()
		if type(length) ~= "number" or length <= 0 or type(gt.gameTime) ~= "number" then return nil end
		return math.floor(gt.gameTime * progression.PER_MONTH / length)
	end)
	if ok then return quarter end
	return nil
end

-- Whether a sample is due: more than one company, and a quarter begun since
-- the last.
function progression.due(saved, quarter)
	if type(saved) ~= "table" or type(quarter) ~= "number" then return false end
	if not progression.multi(saved.companies) then return false end
	local prog = saved.progression
	return type(prog) ~= "table" or prog.quarter == nil or quarter > prog.quarter
end

-- ------------------------------------------------------------ the formulas

-- The game's happiness rating (town_util.getRatingHappinessFromStats), of
-- `unhappy` people of `total` travelling on the company's lines.
function progression.happiness(unhappy, total, sensitivity)
	if sensitivity <= 0 then return 1.0 end
	if total < 15 then total = 15 end
	local average = 1 - (unhappy / total)
	local low = 1 - math.exp(-0.35667 * sensitivity)
	local s = clamp(sensitivity, 0, 1)
	local high = -1.2 * s * s + 2.2 * s
	return clamp((average - low) / (high - low), 0, 1)
end

-- The game's cargo delivery rating (town_util.getRatingCargoDelivery), of
-- `late` items of `delivered` on the company's lines.
function progression.onTime(late, delivered, sensitivity)
	if sensitivity <= 0 then return 1.0 end
	delivered = math.max(10, delivered)
	local fraction = 1 - (late / delivered)
	local low = 1 - math.exp(-0.35667 * sensitivity)
	local s = clamp(sensitivity, 0, 1)
	local high = -1.2 * s * s + 2.2 * s
	return clamp((fraction - low) / (high - low), 0, 1)
end

-- A company's share of a town, 0 to 1: its cargo share and its passenger
-- share weighed by `weights`, a kind nobody carried there left out; nil when
-- nobody carried anything.
function progression.share(cargo, cargoAll, passengers, passengersAll, weights)
	local wc = cargoAll > 0 and (weights.cargo or 0) or 0
	local wp = passengersAll > 0 and (weights.passengers or 0) or 0
	if wc + wp <= 0 then return nil end
	local sum = 0
	if wc > 0 then sum = sum + wc * cargo / cargoAll end
	if wp > 0 then sum = sum + wp * passengers / passengersAll end
	return sum / (wc + wp)
end

-- A company's part of a town: population x share x rating / 100.
function progression.part(population, share, rating)
	return population * share * rating / 100
end

-- ------------------------------------------------------------ the game

-- The game's own modules this needs, in the state it runs in: nil and why
-- when one is missing.
function progression.game(api, require_)
	require_ = require_ or ug_require
	local out = {}
	for key, path in pairs(progression.PATHS) do
		local ok, m = pcall(require_, path)
		if not ok or type(m) ~= "table" then return nil, path .. " did not load: " .. tostring(m) end
		out[key] = m
	end
	local game = {}
	-- The rank `experience` reaches, by the game's thresholds.
	function game.levelFor(experience)
		local base = out.company.getBasePopulation()
		if type(base) ~= "number" or base <= 0 then error("the game has no base population yet", 0) end
		local level = out.progression.getLevelAndFraction(base, experience)
		return level
	end
	-- The game's own progression of a company, as its growth script keeps it.
	function game.own(entity)
		return out.progression.getCompanyProgressionState(entity)
	end
	function game.sensitivity(townState, key)
		return out.towns.getRatingSensitivity(townState, key)
	end
	-- Every town's state, by entity.
	function game.townStates()
		local state = out.towns.externalGetTownsState()
		local byTown = {}
		for _, t in ipairs(type(state) == "table" and state.townStates or {}) do
			local e = type(t.townEntity) == "table" and t.townEntity.entity or t.townEntity
			if type(e) == "number" then byTown[e] = t end
		end
		return byTown
	end
	return game
end

-- The part of a town's rating that is the town's: the lowest of the shared
-- parts the towns script computed last, else its whole rating.
local function sharedRating(townState)
	if type(townState) ~= "table" then return nil end
	local low
	local cached = townState.cachedRatings
	if type(cached) == "table" then
		for _, key in ipairs(progression.SHARED) do
			local r = cached[key]
			local v = type(r) == "table" and r.value or nil
			if type(v) == "number" and (low == nil or v < low) then low = v end
		end
	end
	if low == nil and type(townState.authorityScore) == "number" then low = townState.authorityScore end
	return low
end

-- A statistics entry's delivered and late items: its "all cargo" key (-1)
-- if it has one, else the sum of its cargo types.
local function deliveries(byCargo)
	if type(byCargo) ~= "table" then return 0, 0 end
	local all = byCargo[-1]
	if type(all) == "table" then return all[2] or 0, all[1] or 0 end
	local delivered, late = 0, 0
	for _, pair in pairs(byCargo) do
		if type(pair) == "table" then
			delivered, late = delivered + (pair[2] or 0), late + (pair[1] or 0)
		end
	end
	return delivered, late
end

-- What each company carried for each town, from the game's statistics: a
-- list of towns in entity order, each
--   { town =, population =, state =, carried = { [company id] = { cargo =,
--     late =, passengers =, unhappy = } } }.
function progression.measure(api, roster, game)
	local ids = {}
	for _, c in ipairs(companies.live(roster)) do ids[c.entity] = c.id end
	local owners = {}
	local function companyOf(line)
		if owners[line] == nil then
			local owner = companies.ownerOf(api, line)
			owners[line] = (owner and ids[owner]) or false
		end
		return owners[line] or nil
	end
	local populations = api.engine.system.townBuildingSystem.getTown2personCapacitiesMap()
	local states = game.townStates()
	local halfYear = math.floor(api.util.getDefaultYearDuration() / 2 + 0.5)
	local towns = {}
	for town, capacities in pairs(populations) do
		towns[#towns + 1] = { town = town, population = (type(capacities) == "table" and capacities[1]) or 0 }
	end
	table.sort(towns, function(a, b) return a.town < b.town end)
	for _, t in ipairs(towns) do
		t.state = states[t.town]
		local carried = {}
		local function of(id)
			carried[id] = carried[id] or { cargo = 0, late = 0, passengers = 0, unhappy = 0 }
			return carried[id]
		end
		local stats = api.engine.util.town.getTownDeliveriesStats(t.town, halfYear, true, false)
		for line, byCargo in pairs(stats or {}) do
			local id = type(line) == "number" and line >= 0 and companyOf(line)
			if id then
				local delivered, late = deliveries(byCargo)
				local c = of(id)
				c.cargo, c.late = c.cargo + delivered, c.late + late
			end
		end
		local happiness = api.engine.util.town.getTownHappinessStats(t.town, progression.LINES)
		for _, entry in ipairs(type(happiness) == "table" and happiness.byLine or {}) do
			local id = companyOf(entry[1])
			local by = entry[2]
			if id and type(by) == "table" then
				local c = of(id)
				for _, who in ipairs({ "resident", "nonResident" }) do
					local pair = by[who]
					if type(pair) == "table" then
						c.unhappy = c.unhappy + (pair[1] or 0)
						c.passengers = c.passengers + (pair[2] or 0)
					end
				end
			end
		end
		t.carried = carried
	end
	return towns
end

-- ------------------------------------------------------------ the scores

local function record(prog, id)
	for _, r in ipairs(prog.records) do
		if r.company == id then return r end
	end
	return nil
end

-- The saved record seen when this world's game script starts. These stable
-- company ids and numeric fields let the logs compare a live world with the
-- save a rebase loaded, before a later score can hide a rollback.
function progression.describe(prog, roster, say)
	say = say or function() end
	local live = companies.live(roster)
	say("room ranks loaded: quarter=" .. tostring(prog.quarter) .. " companies=" .. tostring(#live))
	local limit = math.min(#live, 16)
	for i = 1, limit do
		local c = live[i]
		local r = record(prog, c.id)
		if r then
			say("room ranks loaded: company-" .. tostring(c.id) .. " entity=" .. tostring(r.entity)
				.. " experience=" .. tostring(r.experience) .. " potential=" .. tostring(r.potential)
				.. " level=" .. tostring(r.level))
		else
			say("room ranks loaded: company-" .. tostring(c.id) .. " record=missing")
		end
	end
	if #live > limit then say("room ranks loaded: omitted=" .. tostring(#live - limit)) end
end

-- Each live company's record, begun where it has none: the save's own
-- player from the game's own progression, which it earned before the room
-- had companies; any other at rank 1 with nothing, as the game begins one.
local function records(prog, roster, api, game)
	local player = api.engine.util.getPlayer()
	local out = {}
	for _, c in ipairs(companies.live(roster)) do
		local r = record(prog, c.id)
		if r == nil then
			r = { company = c.id, entity = c.entity, experience = 0, potential = 1, level = 1 }
			if c.entity == player then
				local ok, own = pcall(game.own, c.entity)
				if ok and type(own) == "table" then
					r.experience = tonumber(own.experience) or 0
					r.potential = tonumber(own.potentialLevel) or 1
					r.level = tonumber(own.level) or 1
				end
			end
			prog.records[#prog.records + 1] = r
		end
		out[#out + 1] = { company = c, record = r }
	end
	return out
end

-- The passengers' running average of company `id` in `town`, updated with
-- this sample's `count`.
local function averaged(prog, index, id, town, count)
	local key = tostring(id) .. ":" .. tostring(town)
	local entry = index[key]
	if entry == nil then
		entry = { company = id, town = town, value = 0 }
		prog.passengers[#prog.passengers + 1] = entry
		index[key] = entry
	end
	entry.value = entry.value + (count - entry.value) / progression.WINDOW
	entry.seen = true
	return entry.value
end

local function fmt(x) return string.format("%.4f", x) end

-- One sample: every live company's parts of every town and its score, its
-- experience and the rank that reaches. `towns` is measure()'s. `say(line)`
-- is told each town's parts and each company's score. Returns the scores by
-- company id.
function progression.tally(prog, roster, towns, game, api, say, reg, registry, now)
	say = say or function() end
	local weights = prog.weights
	local companiesNow = records(prog, roster, api, game)
	local index = {}
	for _, e in ipairs(prog.passengers) do
		e.seen = false
		index[tostring(e.company) .. ":" .. tostring(e.town)] = e
	end
	local scores = {}
	for _, entry in ipairs(companiesNow) do scores[entry.company.id] = 0 end
	local when = "progression at game time " .. tostring(now)
	say(when .. ": " .. #towns .. " towns, weights cargo " .. tostring(weights.cargo)
		.. " passengers " .. tostring(weights.passengers))
	for _, t in ipairs(towns) do
		-- Every company's passengers here averaged, in roster order.
		local cargoAll, passengersAll = 0, 0
		local averages = {}
		for _, entry in ipairs(companiesNow) do
			local id = entry.company.id
			local c = t.carried[id] or { cargo = 0, late = 0, passengers = 0, unhappy = 0 }
			averages[id] = averaged(prog, index, id, t.town, c.passengers)
			cargoAll = cargoAll + c.cargo
			passengersAll = passengersAll + averages[id]
		end
		local shared = sharedRating(t.state)
		local townId = reg and registry and registry.id(reg, "towns", t.town)
		local name = townId and ("town-" .. tostring(townId)) or ("town entity " .. tostring(t.town))
		if (cargoAll > 0 or passengersAll > 0) and shared == nil then
			say(when .. ": " .. name .. " has no rating this game can read; it counts for nobody")
		elseif cargoAll > 0 or passengersAll > 0 then
			local sensHappy = game.sensitivity(t.state, "people_happiness")
			local sensCargo = game.sensitivity(t.state, "cargo_delivery")
			local parts = {}
			for _, entry in ipairs(companiesNow) do
				local id = entry.company.id
				local c = t.carried[id] or { cargo = 0, late = 0, passengers = 0, unhappy = 0 }
				local share = progression.share(c.cargo, cargoAll, averages[id], passengersAll, weights) or 0
				if share > 0 then
					local own = math.min(shared, progression.happiness(c.unhappy, c.passengers, sensHappy),
						progression.onTime(c.late, c.cargo, sensCargo))
					local rating = 100 * clamp(own, 0, 1)
					local part = progression.part(t.population, share, rating)
					scores[id] = scores[id] + part
					parts[#parts + 1] = "company-" .. id .. " share " .. fmt(share) .. " rating " .. fmt(rating)
						.. " part " .. fmt(part)
				end
			end
			if #parts > 0 then
				say(when .. ": " .. name .. " population " .. tostring(t.population) .. ": " .. table.concat(parts, ", "))
			end
		end
	end
	-- Averages of companies or towns no longer there go.
	local keep = {}
	for _, e in ipairs(prog.passengers) do
		if e.seen and (e.value > 1e-6) then
			e.seen = nil
			keep[#keep + 1] = e
		end
	end
	prog.passengers = keep
	for _, entry in ipairs(companiesNow) do
		local r, id = entry.record, entry.company.id
		local score = scores[id]
		local before = r.experience
		local experience = math.floor(score)
		if experience > r.experience then r.experience = experience end
		local ok, level = pcall(game.levelFor, r.experience)
		if ok and type(level) == "number" then
			r.potential = level
		else
			say(when .. ": company-" .. id .. "'s rank was kept: " .. tostring(level))
		end
		say(when .. ": company-" .. id .. " score " .. fmt(score) .. ", experience " .. tostring(r.experience)
			.. ", rank " .. tostring(r.potential) .. " reached, " .. tostring(r.level) .. " taken"
			.. ", prior experience " .. tostring(before) .. ", sample quarter " .. tostring(prog.quarter))
	end
	return scores
end

-- A sample, for the mod's game script: measures and tallies. Returns true,
-- or false and why; never raises.
local function copyRows(rows)
	local out = {}
	for i, row in ipairs(rows) do
		local copied = {}
		for key, value in pairs(row) do copied[key] = value end
		out[i] = copied
	end
	return out
end

function progression.sample(prog, roster, api, quarter, say, reg, registry, require_)
	if not progression.multi(roster) then return true end
	local game, why = progression.game(api, require_)
	if not game then return false, why end
	local ok, towns = pcall(progression.measure, api, roster, game)
	if not ok then return false, "the towns' statistics did not read: " .. tostring(towns) end
	local now
	pcall(function()
		now = api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME).gameTime
	end)
	-- Only a completed sample consumes its quarter. A missing game module or
	-- unreadable town statistic can recover on the next update of the same
	-- quarter; persisting it early lost that chance and delayed earned XP.
	local copiedRecords, records = pcall(copyRows, prog.records)
	if not copiedRecords then return false, "saved records did not read: " .. tostring(records) end
	local copiedPassengers, passengers = pcall(copyRows, prog.passengers)
	if not copiedPassengers then return false, "saved passengers did not read: " .. tostring(passengers) end
	local candidate = {
		quarter = quarter,
		weights = prog.weights,
		records = records,
		passengers = passengers,
	}
	local tallied, err = pcall(progression.tally, candidate, roster, towns, game, api, say, reg, registry, now)
	if not tallied then return false, tostring(err) end
	prog.quarter = quarter
	prog.records = candidate.records
	prog.passengers = candidate.passengers
	return true
end

-- ------------------------------------------------------------ ranks

-- Company `entity` takes rank `level`, as the growth script's applyLevel
-- lets it: above the rank taken, and reached. Returns true, or false and
-- why.
function progression.take(prog, roster, entity, level)
	if type(level) ~= "number" or level ~= math.floor(level) then return false, "a rank is a whole number" end
	local c = companies.byEntity(roster, entity)
	if not c or c.gone then return false, "the acting company is not in the room's roster" end
	local r = type(prog) == "table" and type(prog.records) == "table" and record(prog, c.id) or nil
	if r == nil then return false, c.name .. " has no rank measured yet" end
	if level <= r.level then return false, c.name .. " has rank " .. r.level .. " already" end
	if level > r.potential then
		return false, c.name .. " has reached rank " .. r.potential .. ", not " .. level
	end
	r.level = level
	return true
end

-- What the game's windows read of company `entity`'s progression
-- (company_progression_util.getCompanyProgressionState), from the mod's
-- game script's state: the rule's record when the room has more than one
-- company, else nil for the game's own.
function progression.view(state, entity)
	if type(state) ~= "table" or not progression.multi(state.companies) then return nil end
	local prog = state.progression
	if type(prog) ~= "table" or type(prog.records) ~= "table" then return nil end
	local c = companies.byEntity(state.companies, entity)
	if not c or c.gone then return nil end
	local r = record(prog, c.id)
	if r == nil then return nil end
	return { experience = r.experience, level = r.level, potentialLevel = r.potential }
end

-- In the GUI's state: the game's windows read the companies' ranks through
-- the view above. `stateOf()` gives the mod's game script's state. Returns
-- true, or false and why.
function progression.follow(stateOf, require_)
	require_ = require_ or ug_require
	local ok, util = pcall(require_, progression.PATHS.progression)
	if not ok or type(util) ~= "table" or type(util.getCompanyProgressionState) ~= "function" then
		return false, "the game's company progression did not load: " .. tostring(util)
	end
	-- Once a module table: another of the GUI's states sharing it changed
	-- it already.
	if util.tpf3mpRanks then return true end
	util.tpf3mpRanks = true
	local original = util.getCompanyProgressionState
	util.getCompanyProgressionState = function(entity, ...)
		local read, mine = pcall(function() return progression.view(stateOf(), entity) end)
		if read and mine then return mine end
		return original(entity, ...)
	end
	return true
end

return progression
