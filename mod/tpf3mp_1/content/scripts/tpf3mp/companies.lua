-- tpf3mp/companies.lua -- the room's companies: which company each player
-- plays for, and the Transport Fever 3 player entity each company is.
--
-- A room starts as one company, the save's own player (company 0), which
-- every player plays for (co-op). A player can found a company of their own
-- (`CompanyOp.Create`), join another (`Join`), rename or recolour the one
-- they play for, and dissolve an empty one: any split, two players in one
-- company and one in another included. Every game applies these at the same
-- update, as every other action of the room's (tpf3mp_sim), so every game
-- keeps the same roster; the mod's game script keeps it in its state, which
-- the game saves with the world, so a player who joins or reloads has it.
--
-- A company is a TF3 player entity (`makeGameAddPlayerCmd(name, colour)`):
-- its money is that entity's ACCOUNT, and what it builds and buys is owned
-- by it (PLAYER_OWNED), as the game keeps ownership. Every game creates it
-- at the same update of the same world, so it is the same entity in every
-- game. What the room orders is booked to the acting player's company
-- (tpf3mp/apply.lua), and what another company owns is refused, the same in
-- every game.
--
-- Who may do what (DECISIONS.md, D22, proposed): a company's players build,
-- buy, run lines, borrow, rename and recolour it; its head (the player who
-- founded it while they play for it, else the one who has played for it
-- longest) alone gives it a password or takes it away, sends a player out of
-- it and opens or closes its stations to other companies' lines: by default,
-- and for single companies on their own (`StationAccess`), per company
-- rather than per player, as a company's players share everything it owns.
-- Joining a company with a password needs it: the room seals the password the player
-- typed (tpf3mp_proto::Secret) and every game compares that seal with the
-- one the company keeps, so no game ever holds the password. The room's
-- first company is everyone's: it has no head, no password, and its
-- stations stay open.
--
--   roster = {
--     next = n,                          -- the next company id
--     list = { { id =, entity =, name =, color = { r, g, b }, gone = true?,
--                founder = "<64 hex digits>"?, lock = { scope =, tag = }?,
--                closed = true?,          -- the default: stations closed
--                access = { { company =, open = }, ... }? }, ... },
--                                        -- its head's choice per company
--     members = { { player = "<64 hex digits>", company = id }, ... },
--                                        -- in the order they joined
--   }
--
-- Lists of records, not tables keyed by id or player: a save keeps them as
-- they are. The ideas come from TpF2 Multiplayer's companies
-- (tpf2-multiplayer by silver2127, mp/companies.lua: companies as engine
-- players, a palette, create/switch/dissolve at their stamp on every
-- machine) and from TPF2MP's ownership rules (tf2mod: rival assets are
-- refused before they apply); the code is new.
--
-- Pure Lua against the game's `api` and a `send` that runs a command at once
-- and returns its data (tpf3mp/apply.lua); the tests give it fakes.

local acceptance = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/acceptance.lua")
    or require("tpf3mp.acceptance")

local companies = {}

-- The most companies a room keeps at once.
companies.MAX = 8

-- The companies' colours, in the order new ones take them: the first no
-- live company has. Plain fractions, as the game's colours.
companies.PALETTE = {
	{ 0.80, 0.16, 0.12 }, -- red
	{ 0.13, 0.42, 0.85 }, -- blue
	{ 0.18, 0.66, 0.27 }, -- green
	{ 0.95, 0.72, 0.08 }, -- yellow
	{ 0.56, 0.27, 0.78 }, -- purple
	{ 0.08, 0.70, 0.72 }, -- teal
	{ 0.95, 0.45, 0.10 }, -- orange
	{ 0.45, 0.45, 0.48 }, -- grey
}

local function copy(color) return { color[1], color[2], color[3] } end

local function vec(api, color) return api.type.Vec3f.new(color[1], color[2], color[3]) end

local function sameColor(a, b)
	return math.abs(a[1] - b[1]) < 1e-3 and math.abs(a[2] - b[2]) < 1e-3 and math.abs(a[3] - b[3]) < 1e-3
end

-- The name the game gives an entity, if it has one.
local function nameOf(api, entity)
	local ok, c = pcall(api.engine.getComponent, entity, api.type.ComponentType.NAME)
	if ok and type(c) == "table" and type(c.name) == "string" and c.name ~= "" then return c.name end
	return nil
end

-- The roster, begun if there is none: company 0, the save's own player,
-- which everyone plays for.
function companies.ensure(roster, api)
	if type(roster) == "table" and type(roster.list) == "table" and type(roster.members) == "table" then
		return roster
	end
	local player = api.engine.util.getPlayer()
	return {
		next = 1,
		list = { { id = 0, entity = player, name = nameOf(api, player) or "Company", color = copy(companies.PALETTE[1]) } },
		members = {},
	}
end

-- The company `id`, gone or not.
function companies.find(roster, id)
	for _, c in ipairs(roster.list) do
		if c.id == id then return c end
	end
	return nil
end

-- The companies still there, in the order they were founded.
function companies.live(roster)
	local out = {}
	for _, c in ipairs(roster.list) do
		if not c.gone then out[#out + 1] = c end
	end
	return out
end

-- The company `player` plays for: the one they joined, else company 0.
function companies.of(roster, player)
	for _, m in ipairs(roster.members) do
		if m.player == player then
			local c = companies.find(roster, m.company)
			if c and not c.gone then return c end
		end
	end
	return companies.find(roster, 0)
end

-- The company whose player entity is `entity`, if any.
function companies.byEntity(roster, entity)
	for _, c in ipairs(roster.list) do
		if c.entity == entity then return c end
	end
	return nil
end

-- The players who play for company `id`.
function companies.members(roster, id)
	local out = {}
	for _, m in ipairs(roster.members) do
		if m.company == id then out[#out + 1] = m.player end
	end
	-- Company 0 also has everyone who never chose; the caller knows who is
	-- in the room, the roster does not.
	return out
end

-- `player` plays for company `id` from now on, last in the join order.
local function setMember(roster, player, id)
	for i, m in ipairs(roster.members) do
		if m.player == player then
			table.remove(roster.members, i)
			break
		end
	end
	roster.members[#roster.members + 1] = { player = player, company = id }
end

-- `player` plays for the room's first company again.
local function leave(roster, player)
	for i, m in ipairs(roster.members) do
		if m.player == player then
			table.remove(roster.members, i)
			return
		end
	end
end

-- The head of company `id`: its founder while they play for it, else the
-- player who has played for it longest; nil for the room's first company,
-- which is everyone's, and for a company nobody plays for.
function companies.head(roster, id)
	if id == 0 then return nil end
	local c = companies.find(roster, id)
	if not c or c.gone then return nil end
	local players = companies.members(roster, id)
	for _, p in ipairs(players) do
		if p == c.founder then return p end
	end
	return players[1]
end

local function stableRoom(room)
	return type(room) == "string" and #room == 34 and room:match("^r%-%x+$") ~= nil
end

-- Password seals are bound to the server's stable RoomId. A display name
-- or invite cannot establish their provenance. Unknown legacy provenance
-- is reset once; subsequent same-room loads preserve the recorded seals.
function companies.roomNeedsBind(roster, room)
	return stableRoom(room) and (type(roster) ~= "table" or roster.passwordRoom ~= room)
end

function companies.bindRoom(roster, room)
	if not stableRoom(room) then return nil, "the stable room identity is unavailable" end
	if roster.passwordRoom == room then return 0 end
	local legacy = not stableRoom(roster.passwordRoom)
	local cleared = {}
	for _, c in ipairs(roster.list or {}) do
		if type(c.lock) == "table" then
			c.lock = nil
			if not c.gone then cleared[#cleared + 1] = c.id end
		end
	end
	roster.passwordRoom = room
	roster.passwordReset = #cleared > 0 and { legacy = legacy, companies = cleared } or nil
	return #cleared
end

function companies.passwordNotice(roster)
	local reset = type(roster) == "table" and roster.passwordReset
	if type(reset) ~= "table" then return nil end
	local names = {}
	for _, id in ipairs(reset.companies or {}) do
		local c = companies.find(roster, id)
		if c and not c.gone then names[#names + 1] = c.name end
	end
	if #names == 0 then return nil end
	local why = reset.legacy and "This older save has no recorded password room."
		or "This save was imported into a different room."
	return why .. " Previous company passwords were cleared and must be set again: " .. table.concat(names, ", ") .. "."
end

local function passwordResolved(roster, id)
	local reset = roster.passwordReset
	if type(reset) ~= "table" then return end
	for i = #(reset.companies or {}), 1, -1 do
		if reset.companies[i] == id then table.remove(reset.companies, i) end
	end
	if #reset.companies == 0 then roster.passwordReset = nil end
end

-- The roster in one line, for hook.log: each live company with its id, its
-- head (the first 8 hex digits), how many chose it, and whether it has a
-- password or closed stations. Never a seal.
function companies.describe(roster)
	local out = {}
	for _, c in ipairs(companies.live(roster)) do
		local head = companies.head(roster, c.id)
		local tags = { #companies.members(roster, c.id) .. " chose it" }
		if head then tags[#tags + 1] = "head " .. head:sub(1, 8) end
		if companies.locked(c) then tags[#tags + 1] = "password" end
		if not companies.open(c) then tags[#tags + 1] = "stations closed" end
		out[#out + 1] = tostring(c.name) .. " #" .. c.id .. " (" .. table.concat(tags, ", ") .. ")"
	end
	return table.concat(out, "; ")
end

-- Whether company `c` has a password.
function companies.locked(c)
	return type(c) == "table" and type(c.lock) == "table"
end

-- Whether other companies' lines may stop at company `c`'s stations: yes
-- unless its head closed them. The default, for every company without a
-- choice of its own (companies.lets).
function companies.open(c)
	return not (type(c) == "table" and c.closed == true)
end

-- Its head's choice for company `other`: true, false, or nil for the
-- default.
function companies.choice(c, other)
	for _, a in ipairs(type(c) == "table" and c.access or {}) do
		if a.company == other then return a.open end
	end
	return nil
end

-- Whether company `other`'s lines may stop at company `c`'s stations: its
-- head's choice for `other`, else the default.
function companies.lets(c, other)
	local choice = companies.choice(c, other)
	if choice ~= nil then return choice end
	return companies.open(c)
end

-- Who owns `entity` (its PLAYER_OWNED player), or nil: the game's own, or
-- no one's.
function companies.ownerOf(api, entity)
	if type(entity) ~= "number" or entity < 0 then return nil end
	-- Native components are userdata on build 40408, while fixtures use
	-- tables. Read the field through the binding instead of discarding it.
	local ok, owner = pcall(function()
		local c = api.engine.getComponent(entity, api.type.ComponentType.PLAYER_OWNED)
		return c and c.player
	end)
	if not ok then return nil end
	if type(owner) ~= "number" or owner < 0 then return nil end
	return owner
end

-- Headquarters, one a company (DECISIONS.md, D22, proposed; docs/HOOKS.md,
-- "Headquarters"). Transport Fever 3 keeps one headquarters a player: its
-- PLAYER component names it (`headquarters`), and its headquarters permit
-- is one at rank 1 (`permitKeys/hq.res.lua`). But the game counts a
-- permit's constructions in the whole world, whoever owns them
-- (`company_util.countUsedConstructionPermits` and the construction menu's
-- `getConstructionDisableCacheData`, game_mechanics/company/company_util.tl,
-- build 40408): once one company has its headquarters, every other
-- company's menu says "Already Built" and its tool "All 1 Permits Used Up".

-- The construction file names whose company metadata says they are
-- headquarters (`metadata.company.headquarters`, as
-- landmarks/hq/headquarter.con declares it), as the game's construction
-- resources say; nil where this game cannot tell.
local hqFiles = {}
function companies.isHeadquarters(api, file)
	if type(file) ~= "string" then return nil end
	if hqFiles[file] ~= nil then return hqFiles[file] end
	local ok, hq = pcall(function()
		local id = api.res.constructionRep.find(file)
		if type(id) ~= "number" or id < 0 then return nil end
		local meta = api.res.constructionRep.get(id).metadata
		local company = meta and meta.company
		return type(company) == "table" and company.headquarters == true
	end)
	if not ok or hq == nil then return nil end
	hqFiles[file] = hq
	return hq
end

-- The headquarters construction `company` (a player entity) owns, if any;
-- nil and why where this game cannot tell. The same in every game: the
-- same world, the same owners.
function companies.headquartersOf(api, company)
	local found, unknown = nil, nil
	local ok, why = pcall(api.engine.forEachEntityWithComponent, function(e)
		if found then return end
		if companies.ownerOf(api, e) ~= company then return end
		local c = api.engine.getComponent(e, api.type.ComponentType.CONSTRUCTION)
		local hq = c and companies.isHeadquarters(api, c.fileName)
		if hq == nil then unknown = c and c.fileName or e
		elseif hq then found = e end
	end, api.type.ComponentType.CONSTRUCTION)
	if not ok then return nil, "this game cannot list the constructions: " .. tostring(why) end
	if found == nil and unknown ~= nil then
		return nil, "this game cannot tell whether " .. tostring(unknown) .. " is a headquarters"
	end
	return found
end

-- Whether `company` may build the construction `file`: anything but a
-- headquarters, and a headquarters while it has none. Else false and why.
-- Every game checks it when the room orders the build.
function companies.mayBuild(roster, company, file, api)
	local hq = companies.isHeadquarters(api, file)
	if hq == nil then return true end
	if not hq then return true end
	local have, why = companies.headquartersOf(api, company)
	if why then return false, why end
	if have then
		local c = roster and companies.byEntity(roster, company)
		return false, (c and c.name or "the company") .. " has its headquarters already"
	end
	return true
end

-- In a GUI Lua state: the game's permit counts count what the player's
-- company owns (PLAYER_OWNED, the company `api.engine.util.getPlayer()`
-- answers there, tpf3mp/follow.lua), not the whole world's, while the room
-- has more than one company (`several()`); with one, the game's own. The
-- same counting as the game's, owner aside; what cannot be counted by
-- owner is counted as the game counts it. The game loads company_util as
-- "/game_mechanics/..." and as "::/game_mechanics/...": each table either
-- gives is changed, once. Returns how many tables it changed, or nil and
-- why.
function companies.followPermits(api, require_, several)
	local okM, meta = pcall(require_, "/game_mechanics/company/company_metadata.tl")
	if not (okM and type(meta) == "table" and type(meta.getKey) == "function") then
		return nil, "the game's company_metadata did not load: " .. tostring(meta)
	end
	local function mine(entity)
		local ok, me = pcall(function() return api.engine.util.getPlayer() end)
		return ok and companies.ownerOf(api, entity) == me
	end
	local function each(withInstances, fn)
		api.engine.system.streetConnectorSystem.forEachConstructionWithMetadata(meta.getKey(), true, withInstances,
			function(entity, construction, id)
				if mine(entity) then fn(entity, construction, id) end
			end)
	end
	local function wrap(util)
		local count, cache = util.countUsedConstructionPermits, util.getConstructionDisableCacheData
		util.countUsedConstructionPermits = function(keys, ...)
			local ok, more = pcall(several)
			if not (ok and more) then return count(keys, ...) end
			local used = {}
			local counted = pcall(each, false, function(_, construction, id)
				local m = api.res.constructionRep.get(id).metadata
				local key = m and util.getActualPermitKey(construction.fileName, m)
				if key then used[key] = 1 + (used[key] or 0) end
			end)
			if not counted then return count(keys, ...) end
			return used
		end
		util.getConstructionDisableCacheData = function(defs, ...)
			local result = cache(defs, ...)
			local ok, more = pcall(several)
			if not (ok and more) or type(result) ~= "table" then return result end
			local data = {}
			local function add(key)
				local entry = data[key]
				if entry then entry.numBuilt = entry.numBuilt + 1 else data[key] = { numBuilt = 1 } end
			end
			local counted = pcall(each, true, function(_, construction)
				if construction.fileName ~= nil then
					add(construction.fileName)
					local instance = meta.constructionInstance and meta.constructionInstance.get(construction.persistentMetadata)
					if instance and instance.permitKey then add(instance.permitKey) end
				end
			end)
			if counted then result.data = data end
			return result
		end
		util.tpf3mpOwnPermits = true
	end
	local changed, already, why, seen = 0, 0, nil, {}
	for _, path in ipairs({ "/game_mechanics/company/company_util.tl", "::/game_mechanics/company/company_util.tl" }) do
		local ok, util = pcall(require_, path)
		if ok and type(util) == "table" and seen[util] then
			-- The same table under its other name.
		elseif ok and type(util) == "table" and type(util.countUsedConstructionPermits) == "function"
				and type(util.getConstructionDisableCacheData) == "function" then
			seen[util] = true
			if not util.tpf3mpOwnPermits then
				wrap(util)
				changed = changed + 1
			else
				already = already + 1
			end
		else
			why = why or ("the game's company_util did not load: " .. tostring(util))
		end
	end
	-- Another GUI state of the same Lua state changed it first: it counts
	-- each company's own already.
	if changed == 0 and already > 0 then return already end
	if changed == 0 then return nil, why or "the game's company_util did not load" end
	return changed + already
end

-- What each live company owns, as the engine records it (PLAYER_OWNED), in
-- one line for hook.log: its constructions and its headquarters. Read
-- only. Written once a world is up, so a world loaded from a save says
-- whether its owners came back with it (a company owning nothing it
-- built is an owner lost); nil where this game cannot list them.
function companies.ownership(roster, api)
	local counts = {}
	local ok = pcall(api.engine.forEachEntityWithComponent, function(e)
		local owner = companies.ownerOf(api, e)
		if owner then counts[owner] = (counts[owner] or 0) + 1 end
	end, api.type.ComponentType.CONSTRUCTION)
	if not ok then return nil end
	local out = {}
	for _, c in ipairs(companies.live(roster)) do
		local hq = companies.headquartersOf(api, c.entity)
		out[#out + 1] = tostring(c.name) .. " #" .. c.id .. " (entity " .. tostring(c.entity) .. "): "
			.. (counts[c.entity] or 0) .. " construction(s)" .. (hq and (", headquarters " .. hq) or "")
	end
	return table.concat(out, "; ")
end

-- What a headquarters gives, and to whom (docs/HOOKS.md, "Headquarters").
-- Build 40408 keeps no headquarters bonus per company: the game's town
-- script (game_mechanics/towns/towns.script.tl, updateConstructions, every
-- 20 updates) sums the `town_growth` instance metadata of every
-- construction in the world, whoever owns it, onto the town closest to it
-- (landmarks/landmark_util.tl, collectTownGrowthMetadata), and that town's
-- experience grows by that much more (town_util.getXpFactor: 1 +
-- xpIncrease). A headquarters carries xp +5% itself and +1% for each
-- medium wing, and reputation recovery +1% for each large wing
-- (landmarks/hq/headquarter.script.tl, headquarter_addon.script.tl, the
-- modules' metadata). So each company's headquarters already gives its
-- town what a single player's gives, in every game alike; the mod adds
-- nothing to it. Its PLAYER `headquarters` the engine sets for the
-- proposal's `playerEntity` (apply_proposal.cpp, "ce.playerEntity !=
-- ecs::Entity()"), which only the GUI reads: the capital badge, the
-- "Headquarters" tooltip and selection.
companies.TOWN_SCRIPT = "::/game_mechanics/towns/town.gs"

local function number(v)
	if type(v) ~= "number" then return 0 end
	return v
end

-- For hook.log, read only: one line per live company whose PLAYER names a
-- headquarters, saying which construction it is and who owns it, the town
-- it is closest to, the bonus on it, and the bonus the game's town script
-- applies to that town. Bounded work that never waits: at most
-- `REPORT_MAX` companies, a few engine reads each, no pass over the world's
-- constructions, and nothing at all read while no company has one; every
-- read in a pcall. Nil and why where the roster is not readable.
companies.REPORT_MAX = 8
function companies.headquartersReport(roster, api)
	if type(roster) ~= "table" or type(roster.list) ~= "table" then return nil, "no roster" end
	local found = {}
	for _, c in ipairs(companies.live(roster)) do
		if #found >= companies.REPORT_MAX then break end
		local hq = nil
		pcall(function()
			local p = api.engine.getComponent(c.entity, api.type.ComponentType.PLAYER)
			hq = p and p.headquarters
		end)
		if type(hq) == "number" and hq >= 0 then found[#found + 1] = { c = c, hq = hq } end
	end
	local out = {}
	if #found == 0 then return out end
	local towns = nil
	pcall(function()
		local entity = api.engine.system.gameScriptSystem.getEntityForGameScript(companies.TOWN_SCRIPT)
		if type(entity) ~= "number" or entity < 0 then return end
		local script = api.engine.getComponent(entity, api.type.ComponentType.GAME_SCRIPT)
		towns = script and script.state and script.state.townStates
	end)
	for _, f in ipairs(found) do
		local c, hq = f.c, f.hq
		local owner, town, name, xp, recovery, built = nil, nil, nil, 0, 0, false
		pcall(function() owner = companies.ownerOf(api, hq) end)
		pcall(function()
			local con = api.engine.getComponent(hq, api.type.ComponentType.CONSTRUCTION)
			if not con then return end
			built = true
			local growth = con.persistentMetadata and con.persistentMetadata.town_growth
			if growth then xp, recovery = number(growth.xpIncrease), number(growth.reputationRecoveryBoost) end
		end)
		if built then
			pcall(function()
				local t = api.engine.system.streetConnectorSystem.getConstructionClosestTown(hq)
				if type(t) == "number" and t >= 0 then town = t end
			end)
		end
		if town then pcall(function() name = api.engine.util.getEntityName(town) end) end
		local applied = "the game's town script has no state for that town"
		if town and type(towns) == "table" then
			for k = 1, math.min(#towns, 4096) do
				local t = towns[k]
				local te = type(t) == "table" and t.townEntity
				if type(te) == "table" and te.entity == town and type(t.constructionBoni) == "table" then
					applied = string.format("the game's town script applies xp +%.2f, reputation recovery +%.2f there",
						number(t.constructionBoni.xpIncrease), number(t.constructionBoni.reputationRecoveryBoost))
					break
				end
			end
		end
		out[#out + 1] = string.format("%s #%s: headquarters %s (%s), owned by %s; closest town %s%s: "
			.. "on it xp +%.2f, reputation recovery +%.2f; %s",
			tostring(c.name), tostring(c.id), tostring(hq), built and "a construction" or "no construction",
			tostring(owner), tostring(town), name and (" (" .. tostring(name) .. ")") or "", xp, recovery, applied)
	end
	return out
end

-- Whether `company` (a player entity) may change `entity`: what no company
-- owns, and what it owns itself. Else false and why, naming the owner.
function companies.mayTouch(roster, company, entity, api, what)
	local owner = companies.ownerOf(api, entity)
	if owner == nil or owner == company then return true end
	local other = roster and companies.byEntity(roster, owner)
	local name = other and other.name or "another company"
	return false, "the " .. (what or "thing") .. " belongs to " .. name
end

-- A company's vehicles use its own depots (DECISIONS.md, D22 station-access
-- decision). Unlike mayTouch, a depot with no readable owner is not usable in
-- a multi-company room: do not let a missing PLAYER_OWNED component turn into
-- permission. With one company, keep the game's native purchase behavior.
function companies.mayBuyAtDepot(roster, company, depot, api)
	if not companies.painting(roster) then return true end
	local owner = companies.ownerOf(api, depot)
	if owner == company then return true end
	if owner == nil then return false, "the depot has no company owner" end
	local other = roster and companies.byEntity(roster, owner)
	return false, "the depot belongs to " .. (other and other.name or "another company")
end

-- Whether `company` (a player entity) may have its lines stop at the
-- station group `group` (D22, proposed): one no company owns, its own, or
-- another company's that keeps its stations open. Else false and why,
-- naming the owner. Stopping at a station changes nothing of it, so it is
-- not `mayTouch`'s.
function companies.mayUse(roster, company, group, api)
	local owner = companies.ownerOf(api, group)
	if owner == nil or owner == company then return true end
	local other = roster and companies.byEntity(roster, owner)
	local mine = roster and companies.byEntity(roster, company)
	if other and not companies.lets(other, mine and mine.id) then
		if mine and companies.choice(other, mine.id) == false then
			return false, "the station belongs to " .. other.name .. ", which keeps its stations from " .. mine.name
		end
		return false, "the station belongs to " .. other.name .. ", which keeps its stations to itself"
	end
	return true
end

-- The line manager runs in more than one GUI Lua state. Install the same
-- station predicate in each state's entity_util, including the HUD state.
-- Resolve the company from the room roster; native GUI ownership can still
-- refer to the save's original player. Never extend this to depots/assets.
local stationUtilities = setmetatable({}, { __mode = "k" })
local stationSelections = setmetatable({}, { __mode = "k" })
function companies.followStations(api, require_, current, inHudState)
	local changed, seen = 0, {}
	for _, path in ipairs({ "/scripts/entity_util.tl", "::/scripts/entity_util.tl" }) do
		local ok, util = pcall(require_, path)
		if ok and type(util) == "table" and stationUtilities[util] and not seen[util] then
			changed = changed + 1
		end
		if ok and type(util) == "table" and not stationUtilities[util]
			and type(util.isOwnedByPlayerOrNotOwned) == "function" then
			local original = util.isOwnedByPlayerOrNotOwned
			util.isOwnedByPlayerOrNotOwned = function(entity, ...)
				local roster, me = current()
				if roster and me then
					local known, station = pcall(function()
						local CT = api.type.ComponentType
						if api.engine.getComponent(entity, CT.STATION_GROUP) ~= nil then return true end
						local c = api.engine.getComponent(entity, CT.CONSTRUCTION)
						return c ~= nil and c.stations ~= nil and #c.stations > 0
					end)
					if known and station then
						local mine = companies.of(roster, me)
						if not mine or not mine.entity then return false end
						return companies.mayUse(roster, mine.entity, entity, api)
					end
				end
				return original(entity, ...)
			end
			stationUtilities[util] = true
			changed = changed + 1
		end
		if ok and type(util) == "table" then seen[util] = true end
	end
	-- line_util loads React's builtin recipes. Only the HUD state has that
	-- registry; requiring it from the game-script GUI produces native errors.
	if not inHudState then return changed end
	-- Build 40408's native selector can return a STATION entity with
	-- TransportNetworkEdge details for a station built by another engine
	-- player. The line manager then treats even our own company's station as
	-- a waypoint. Recover the ordinary station details from that entity;
	-- leave genuine network edges and detailed terminal selections alone.
	local ok, line = pcall(require_, "/gui/line_vehicle_mgmt/line_util.tl")
	if ok and type(line) == "table" and not stationSelections[line]
		and type(line.convertDetails) == "function" then
		local original = line.convertDetails
		line.convertDetails = function(entity, details)
			local converted = original(entity, details)
			local roster, me = current()
			if roster and me and converted and converted.transportNetworkEdge then
				local read, group = pcall(function()
					local CT = api.type.ComponentType
					if api.engine.getComponent(entity, CT.STATION_GROUP) then return entity end
					if api.engine.getComponent(entity, CT.STATION) then
						return api.engine.system.stationGroupSystem.getStationGroup(entity)
					end
				end)
				if read and type(group) == "number" and group >= 0 then
					local mine = companies.of(roster, me)
					if not mine or not mine.entity or not companies.mayUse(roster, mine.entity, group, api) then
						return nil
					end
					return original(entity, nil)
				end
			end
			return converted
		end
		stationSelections[line] = true
	end
	return changed
end

-- Whether anything is owned by the player entity `entity`; nil when this
-- game cannot list what players own.
-- The callback is given the entity only (build 40408; TPF2's also had the
-- component), so each one's owner is read, and never raises: an error in the
-- callback ends the game.
function companies.owns(api, entity)
	local owned = {}
	local ok = pcall(api.engine.forEachEntityWithComponent, function(e)
		owned[#owned + 1] = e
	end, api.type.ComponentType.PLAYER_OWNED)
	if not ok then return nil end
	for _, e in ipairs(owned) do
		if companies.ownerOf(api, e) == entity then return true end
	end
	return false
end

local function trimmed(name)
	if type(name) ~= "string" then return nil end
	name = name:gsub("^%s+", ""):gsub("%s+$", "")
	if name == "" then return nil end
	return name
end

local function nameTaken(roster, name, except)
	local lower = name:lower()
	for _, c in ipairs(companies.live(roster)) do
		if c.id ~= except and c.name:lower() == lower then return true end
	end
	return false
end

local function freeColor(roster)
	for _, color in ipairs(companies.PALETTE) do
		local used = false
		for _, c in ipairs(companies.live(roster)) do
			if sameColor(c.color, color) then used = true break end
		end
		if not used then return copy(color) end
	end
	return copy(companies.PALETTE[#companies.PALETTE])
end


local function memberOf(roster, player, id)
	return companies.of(roster, player).id == id
end

-- Refuses unless `player` heads company `c`.
local function headOf(roster, player, c, doing)
	if c.id == 0 then return false, "the room's first company is everyone's: nobody " .. doing .. " it" end
	if companies.head(roster, c.id) ~= player then
		return false, "only the head of " .. c.name .. " " .. doing .. " it"
	end
	return true
end

-- Whether `seal` (the room's, { scope =, tag = }) is a password's for
-- company `id`.
local function sealFor(seal, id)
	return type(seal) == "table" and seal.scope == id and type(seal.tag) == "string" and #seal.tag == 64
end

-- A colour as the action carries it, { r =, g =, b = } in fractions, or nil
-- for one out of range.
local function colorOf(color)
	if type(color) ~= "table" then return nil end
	local out = { color.r, color.g, color.b }
	for k = 1, 3 do
		local v = out[k]
		if type(v) ~= "number" or v ~= v or v < 0 or v > 1 then return nil end
	end
	return out
end

-- The entity a command made, from its data's `field`, else its first result.
local function made(data, entities, field)
	local ok, e = pcall(function() return data[field] end)
	if ok and type(e) == "number" and e >= 0 then return e end
	local first = type(entities) == "table" and entities[1]
	e = type(first) == "table" and first[1] or nil
	if type(e) == "number" and e >= 0 then return e end
	return nil
end

-- ------------------------------------------------------------- colours
--
-- A company's colour is its vehicles': with more than one company in the
-- room, a vehicle it buys is painted in it (tpf3mp/apply.lua), and a new
-- colour repaints all of them, so a glance tells whose a bus is. With one
-- company the game's own colours stay, as in single player.

-- Whether vehicles take their company's colour: more than one company.
function companies.painting(roster)
	return type(roster) == "table" and #companies.live(roster) > 1
end

-- The index of the palette colour `color` is ({ r, g, b }, within what a
-- float keeps of it), or nil for any other colour. A vehicle painted in it
-- has its marker on the map in it too (gui/tpf3mp/tpf3mp.css.lua has a
-- class for each).
function companies.swatch(color)
	if type(color) ~= "table" then return nil end
	for i, p in ipairs(companies.PALETTE) do
		local same = true
		for k = 1, 3 do
			local v = color[k]
			if type(v) ~= "number" then return nil end
			if math.abs(v - p[k]) > 0.01 then same = false end
		end
		if same then return i end
	end
	return nil
end

-- The style class of the markers of vehicles in palette colour `index`.
function companies.markerClass(index)
	return "tpf3mp-company-" .. tostring(index)
end

-- The style class of the town label of a capital in palette colour
-- `index` (tpf3mp/capitals.lua; gui/tpf3mp/tpf3mp.css.lua).
function companies.capitalClass(index)
	return "tpf3mp-capital-" .. tostring(index)
end

-- The mod's game script, by the names the game gives it: game scripts are
-- entities, named by their file (the game's loan window finds the loan
-- script so).
companies.SCRIPTS = { "tpf3mp_1::/tpf3mp_sim/tpf3mp_sim.gs", "tpf3mp_1::/tpf3mp_sim.gs" }

-- The mod's game script's state as the game keeps it (the roster is its
-- `companies`), read from any GUI Lua state; nil before there is one.
function companies.scriptState(api)
	for _, name in ipairs(companies.SCRIPTS) do
		local ok, state = pcall(function()
			local entity = api.engine.system.gameScriptSystem.getEntityForGameScript(name)
			if type(entity) ~= "number" or entity < 0 then return nil end
			local c = api.engine.getComponent(entity, api.type.ComponentType.GAME_SCRIPT)
			return c and c.state
		end)
		if ok and type(state) == "table" then return state end
	end
	return nil
end

-- Paints `vehicle` in company `c`'s colour.
function companies.paintVehicle(c, vehicle, send, api)
	local color = c and c.color
	if type(color) ~= "table" or type(vehicle) ~= "number" then return end
	send(api.cmd.makeEntitySetColorCmd(vehicle, vec(api, color)))
end

-- Repaints every vehicle company `c` owns, in the engine's own order (the
-- same entities in the same order in every game). Returns how many.
function companies.paintFleet(c, send, api)
	local vehicles = {}
	pcall(api.engine.forEachEntityWithComponent, function(e)
		local owner = companies.ownerOf(api, e)
		if owner == c.entity then vehicles[#vehicles + 1] = e end
	end, api.type.ComponentType.TRANSPORT_VEHICLE)
	for _, e in ipairs(vehicles) do companies.paintVehicle(c, e, send, api) end
	return #vehicles
end

-- ------------------------------------------------------------- loans
--
-- The game's loan script (::/game_mechanics/finance/loan.gs) keeps the
-- loans of the room's first company only: it books them to the save's own
-- player. Another company's loans are the room's: taken on the same terms
-- the game offers (the loan script's availableLoans), booked to that
-- company as the game books a loan (a LOAN journal entry, which raises the
-- account's balance and its loan alike, seen on build 40408), and paid back
-- with the native loan_util.tl's equal principal and fixed interest, with
-- rounding corrections on the final payment; or all at once. Each loan's
-- payment origin is its own game timestamp, as in loan.script.tl.
--
--   roster.loans = { { id =, company =, amount =, remaining =, months =,
--                      duration =, percentage =, paid =, lastPayDay =,
--                      accounting = "native-v1", payment = }, ... }
--   roster.nextLoan = n
--   roster.loanOffers = { { company = id, availableLoans = { Loan, ... } }, ... }

companies.LOAN_SCRIPT = "::/game_mechanics/finance/loan.gs"

local function loanState(api)
	local ok, state = pcall(function()
		local entity = api.engine.system.gameScriptSystem.getEntityForGameScript(companies.LOAN_SCRIPT)
		if type(entity) ~= "number" or entity < 0 then return nil end
		local component = api.engine.getComponent(entity, api.type.ComponentType.GAME_SCRIPT)
		return component and component.state
	end)
	return ok and type(state) == "table" and state or nil
end

local function copyOffer(offer)
	if type(offer) ~= "table" then return nil end
	local out = {}
	for key, value in pairs(offer) do out[key] = value end
	return out
end

local function loanOfferGroup(roster, id)
	for _, group in ipairs(type(roster) == "table" and roster.loanOffers or {}) do
		if type(group) == "table" and group.company == id then return group end
	end
	return nil
end

local function createLoan(api, kind)
	if type(ug_require) ~= "function" or type(kind) ~= "string" then return nil end
	local ok, util = pcall(ug_require, "::/game_mechanics/finance/loan_util.tl")
	if not ok or type(util) ~= "table" then return nil end
	local make = util["create" .. kind .. "Loan"]
	if type(make) ~= "function" then return nil end
	ok, util = pcall(make)
	if not ok or type(util) ~= "table" or util.type ~= kind or type(util.amount) ~= "number"
		or type(util.duration) ~= "number" or type(util.percentage) ~= "number" then return nil end
	return copyOffer(util)
end

-- A founded company starts with its own copy of the native loan offers. A
-- native cooldown belongs only to the save's player, so draw a fresh offer
-- of that kind from the game's utility, as the native loan update does.
-- Called from the ordered simulation update, where the room has seeded
-- math.random identically in every game.
local function seedLoanOffers(roster, id, api)
	if id == 0 or loanOfferGroup(roster, id) then return loanOfferGroup(roster, id) end
	local real = loanState(api)
	if type(real) ~= "table" or type(real.availableLoans) ~= "table" then return nil end
	local offers = {}
	for i, source in ipairs(real.availableLoans) do
		local offer = copyOffer(source)
		if offer and offer.cooldownUntil ~= nil then
			offer = createLoan(api, offer.type) or offer
		end
		if offer then offers[i] = offer end
	end
	if #offers == 0 then return nil end
	roster.loanOffers = roster.loanOffers or {}
	local group = { company = id, availableLoans = offers }
	roster.loanOffers[#roster.loanOffers + 1] = group
	return group
end

-- Seed companies on the simulation side, including a roster saved before
-- company offers became persistent. GUI reads never mutate the roster.
function companies.ensureLoanOffers(roster, api)
	if type(roster) ~= "table" then return false end
	local changed = false
	for _, c in ipairs(companies.live(roster)) do
		if c.id ~= 0 and not loanOfferGroup(roster, c.id) then
			if seedLoanOffers(roster, c.id, api) then changed = true end
		end
	end
	return changed
end

-- Keep the simulation update alive when an old roster needs its offers
-- initialized, or when one of its independent cooldowns expires.
function companies.loanOffersNeedInit(roster, api)
	local real = loanState(api)
	if type(roster) ~= "table" or type(real) ~= "table" or type(real.availableLoans) ~= "table" then return false end
	for _, c in ipairs(companies.live(roster)) do
		if c.id ~= 0 and not loanOfferGroup(roster, c.id) then return true end
	end
	return false
end

local function gameTimeNow(api)
	local ok, gameTime = pcall(function()
		local world = api.engine.util.getWorld()
		local time = api.engine.getComponent(world, api.type.ComponentType.GAME_TIME)
		return time and time.gameTime
	end)
	return ok and type(gameTime) == "number" and gameTime or nil
end

function companies.loanOffersDue(roster, api)
	local now = gameTimeNow(api)
	if now == nil then return false end
	for _, group in ipairs(type(roster) == "table" and roster.loanOffers or {}) do
		for _, offer in ipairs(type(group) == "table" and group.availableLoans or {}) do
			if type(offer) == "table" and type(offer.cooldownUntil) == "number" and offer.cooldownUntil < now then
				return true
			end
		end
	end
	return false
end

function companies.refreshLoanOffers(roster, api)
	local now = gameTimeNow(api)
	if now == nil then return false, "this game does not say what time it is" end
	local changed = false
	for _, group in ipairs(type(roster) == "table" and roster.loanOffers or {}) do
		for i, offer in ipairs(type(group) == "table" and group.availableLoans or {}) do
			if type(offer) == "table" and type(offer.cooldownUntil) == "number" and offer.cooldownUntil < now then
				local fresh = createLoan(api, offer.type)
				if not fresh then return false, "the game's loan utility cannot replace a cooled-down offer" end
				group.availableLoans[i] = fresh
				changed = true
			end
		end
	end
	return changed
end

-- The month of the game's calendar now, counted from the game's start; nil
-- where the game does not say.
function companies.monthNow(api)
	local ok, month = pcall(function()
		local gt = api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME)
		local length = api.util.getDefaultMonthDuration()
		if type(length) ~= "number" or length <= 0 or type(gt.gameTime) ~= "number" then return nil end
		return math.floor(gt.gameTime / length), length
	end)
	if ok then return month end
	return nil
end

local function monthLength(api)
	local ok, length = pcall(function() return api.util.getDefaultMonthDuration() end)
	if ok and type(length) == "number" and length > 0 then return length end
	return nil
end

-- Books `amount` (negative: taken from it) to the company `entity`, as a
-- LOAN or INTEREST entry.
local function book(api, send, entity, amount, kind)
	local entry = api.type.JournalEntry.new()
	entry.amount = amount
	entry.time = -1
	entry.category.type = api.type.JournalEntry.Type[kind]
	send(api.cmd.makeJournalBookAssetCmd(entity, entry))
end

-- Native loan_util.tl:getMonthlyLoanAndInterestWithErrors (build 40408).
-- percentage is interest over the whole term, not an annual rate. Keep
-- both rounding corrections: even a negative interest correction is
-- booked by the native script on the final payment.
local function nativePayments(amount, months, percentage)
	local monthly = amount / months
	local principal = math.floor(monthly + 0.5)
	local interest = math.floor(percentage * monthly + 0.5)
	return principal, interest, amount - months * principal,
		math.floor(amount * percentage + 0.5) - months * interest
end

-- Saved annuity loans have no per-loan time origin. Preserve the exact
-- outstanding debt and all previously booked journals, and refinance only
-- that balance over the unexpired number of payments at the offered term
-- percentage. This changes no money. The new schedule starts now, once,
-- so neither a stale roster.month nor an old paid count charges it again.
function companies.migrateLoans(roster, api)
	local now, length = gameTimeNow(api), monthLength(api)
	if now == nil or length == nil then return 0 end
	local migrated, keep = 0, {}
	for _, loan in ipairs(roster.loans or {}) do
		if loan.accounting ~= "native-v1" and loan.remaining > 0 then
			loan.legacy = { amount = loan.amount, remaining = loan.remaining, months = loan.months,
				paid = loan.paid, rate = loan.rate, payment = loan.payment }
			loan.amount = loan.remaining
			loan.months = math.max(1, loan.months - loan.paid)
			loan.duration = loan.months * length
			loan.percentage = loan.rate * 12
			loan.paid, loan.lastPayDay, loan.accounting = 0, now, "native-v1"
			local principal, interest = nativePayments(loan.amount, loan.months, loan.percentage)
			loan.payment = principal + interest
			loan.rate = nil
			migrated = migrated + 1
		end
		-- Native rounding corrections may temporarily make the principal
		-- zero or negative before the final installment; its final principal
		-- and interest corrections still have to be booked.
		if loan.accounting == "native-v1" and loan.paid < loan.months then keep[#keep + 1] = loan end
	end
	roster.loans = keep
	roster.month = nil
	return migrated
end

-- The loans of company `id`.
function companies.loansOf(roster, id)
	local out = {}
	for _, loan in ipairs(roster.loans or {}) do
		if loan.company == id then out[#out + 1] = loan end
	end
	return out
end

-- The most loans a company has at once, as the game's loan script allows
-- (loan_util.tl, maximalObtainableLoans).
companies.MAX_LOANS = 4

-- The loan script's state (LoanTable, loan.d.tl) as the game's finance
-- window should show it to a player of company `id`, not the room's first:
-- the company's persisted offers and loans, each by its room id and amount,
-- which its Repay sends back (companies.repay). The first company continues
-- to use the game's own loan state. `monthLength` is the game's.
function companies.loanTable(roster, id, real, monthLength, fresh)
	local offers = {}
	local group = id ~= 0 and loanOfferGroup(roster, id) or nil
	local source = group and group.availableLoans
	if not group and id == 0 then source = type(real) == "table" and real.availableLoans end
	for _, offer in ipairs(type(source) == "table" and source or {}) do
		local copyOfOffer = copyOffer(offer)
		if copyOfOffer then offers[#offers + 1] = copyOfOffer end
	end
	local obtained = {}
	for _, loan in ipairs(type(roster) == "table" and companies.loansOf(roster, id) or {}) do
		-- GUI reads never migrate simulation state or invent a payoff for an
		-- old annuity loan. The simulation migrates it at its next update.
		if loan.accounting ~= "native-v1" then error("the company's loan accounting is awaiting migration") end
		obtained[#obtained + 1] = { type = loan.type or "Custom", amount = loan.amount,
			duration = loan.duration, percentage = loan.percentage,
			timesPaid = loan.paid, id = loan.id }
	end
	return { availableLoans = offers, obtainedLoans = obtained, freeId = type(roster) == "table" and roster.nextLoan or 1 }
end

local function sameOffer(offer, terms)
	return type(offer) == "table" and type(terms) == "table"
		and offer.type == terms.type and offer.amount == terms.amount
		and offer.duration == terms.duration and offer.percentage == terms.percentage
end

-- Company `id` takes an exact offer in its own slot. `next` is the fresh
-- loan the native finance window draws before clicking; like loan.script.tl,
-- only its type identifies the slot that goes on cooldown.
function companies.borrow(roster, id, terms, nextTerms, send, api)
	local c = companies.find(roster, id)
	if not c or c.gone then return false, "there is no such company" end
	if #companies.loansOf(roster, id) >= companies.MAX_LOANS then
		return false, c.name .. " has " .. companies.MAX_LOANS .. " loans already"
	end
	local state = loanOfferGroup(roster, id) or seedLoanOffers(roster, id, api)
	if not state or type(state.availableLoans) ~= "table" then return false, "this company's loan offers are not available" end
	if type(terms) ~= "table" or type(terms.type) ~= "string" or type(terms.amount) ~= "number"
		or type(terms.duration) ~= "number" or type(terms.percentage) ~= "number" then
		return false, "a loan needs a current offered term"
	end
	if type(nextTerms) ~= "table" or nextTerms.type ~= terms.type then
		return false, "the replacement must match the offered loan type"
	end
	local slot
	for i, offer in ipairs(state.availableLoans) do
		if sameOffer(offer, terms) and offer.cooldownUntil == nil then slot = i break end
	end
	if not slot then return false, "that loan offer is no longer available" end
	local length = monthLength(api)
	if not length then return false, "this game does not say how long a month is" end
	local amount, duration, percentage = terms.amount, terms.duration, terms.percentage
	if amount <= 0 or duration <= 0 or percentage < 0 then return false, "the offered loan terms are invalid" end
	local months = math.max(1, math.floor(duration / length + 0.5))
	local now = gameTimeNow(api)
	if now == nil then return false, "this game does not say what time it is" end
	local minCooldown, maxCooldown = length * 4, length * 8
	if maxCooldown > 2147483647 then return false, "this game's loan cooldown is out of range" end
	local cooldown = math.random(minCooldown, maxCooldown)
	book(api, send, c.entity, amount, "LOAN")
	state.availableLoans[slot] = { type = terms.type, cooldownUntil = now + cooldown }
	roster.loans = roster.loans or {}
	roster.nextLoan = (roster.nextLoan or 1)
	local principal, interest = nativePayments(amount, months, percentage)
	roster.loans[#roster.loans + 1] = { id = roster.nextLoan, company = id, amount = amount, remaining = amount,
		months = months, duration = duration, percentage = percentage, paid = 0, lastPayDay = now,
		accounting = "native-v1", payment = principal + interest, type = terms.type }
	roster.nextLoan = roster.nextLoan + 1
	return true
end

-- Company `id` pays its loan back, all that is still owed.
-- `terms` names the loan by its id and amount: the game's finance window
-- lists the loan script's loans, the room's first company's, whose ids
-- count from 0 as the room's own count from 1, so an id alone could name
-- another loan of this company's; one whose amount differs is refused.
function companies.repay(roster, id, terms, send, api)
	local loanId = type(terms) == "table" and terms.id or nil
	local amount = type(terms) == "table" and tonumber(terms.amount) or nil
	for i, loan in ipairs(roster.loans or {}) do
		if loan.id == loanId and loan.company == id then
			if amount ~= loan.amount then
				return false, "that loan is not this company's: its own are in the Multiplayer window"
			end
			local c = companies.find(roster, id)
			book(api, send, c.entity, -loan.remaining, "LOAN")
			table.remove(roster.loans, i)
			return true
		end
	end
	return false, "the company has no such loan"
end

-- At most one installment per due loan per update, as loan.script.tl:
-- lastPayDay becomes the game's current time, not a global month index.
-- A delayed update never charges time before the loan was taken.
function companies.chargeLoans(roster, send, api)
	local now = gameTimeNow(api)
	if now == nil then return 0 end
	local payments, keep = 0, {}
	for _, loan in ipairs(roster.loans or {}) do
		if loan.accounting ~= "native-v1" then error("loan accounting must be migrated before charging") end
		if loan.lastPayDay + loan.duration / loan.months <= now then
			local principal, interest, principalError, interestError = nativePayments(loan.amount, loan.months, loan.percentage)
			loan.paid = loan.paid + 1
			loan.lastPayDay = now
			if loan.paid == loan.months then
				principal, interest = principal + principalError, interest + interestError
			end
			local c = companies.find(roster, loan.company)
			if c then
				book(api, send, c.entity, -principal, "LOAN")
				book(api, send, c.entity, -interest, "INTEREST")
			end
			loan.remaining = loan.remaining - principal
			payments = payments + 1
		end
		if loan.paid < loan.months then keep[#keep + 1] = loan end
	end
	roster.loans = keep
	return payments
end

-- Old schedules also wake the simulation so they migrate even without a
-- player action. Reading this predicate never changes the saved roster.
function companies.loansDue(roster, api)
	local now = gameTimeNow(api)
	if type(roster) ~= "table" or now == nil or monthLength(api) == nil then return false end
	for _, loan in ipairs(roster.loans or {}) do
		if loan.accounting ~= "native-v1" or loan.lastPayDay + loan.duration / loan.months <= now then return true end
	end
	return false
end

-- ------------------------------------------------------------- subsidies
--
-- The game's subsidy script (::/game_mechanics/subventions/subventions.gs,
-- build 40408's subventions.script.tl) draws the offers in its update, from
-- the world and the game time, its math.random reseeded per call by the hook
-- (docs/HOOKS.md, "Seeds, as built"): alike in every game only while their
-- worlds and game times agree at every room step, which the economy lane
-- checks (tpf3mp/lanes.lua, tpf3mp/subsidies.lua). It keeps them in
-- its state, offered (`proposedSubventions`), taken (`activeSubventions`),
-- completed and failed, each by its number (`uid`) and its kind (`id`, the
-- subsidy resource). Accepting moves an offer to the taken and books its
-- money up front; the script books the money for completing it, or the
-- penalty for failing it, itself, months later. It books all of it to the
-- save's own player (subvention_util.tl, applyBonusMalus: getPlayer()),
-- which in a game script's state is the room's first company.
--
-- So a subsidy another company takes is the room's to settle: every game
-- moves what the script booked to the first company on to the company that
-- took it, as SUBSIDY journal entries (the first company's books show the
-- money in and out again, so they net to nothing), at accepting and when the
-- script completes or fails it, at the same update in every game. Only the
-- taker's transport counts towards it (tpf3mp/subsidies.lua).
--
--   roster.subsidies = { { uid =, kind =, company =, state = "taken" |
--                          "completed" }, ... }
--   roster.subsidyDay = the last game day the subsidies were settled

companies.SUBSIDY_SCRIPT = "::/game_mechanics/subventions/subventions.gs"

-- The subsidy script's lists, and what each says of a subsidy in it.
local SUBSIDY_LISTS = {
	{ "proposedSubventions", "offered" },
	{ "activeSubventions", "taken" },
	{ "completedSubventions", "completed" },
	{ "failedSubventions", "failed" },
}

-- The subsidy script's state as the game keeps it, or nil.
function companies.subsidyState(api)
	local ok, state = pcall(function()
		local entity = api.engine.system.gameScriptSystem.getEntityForGameScript(companies.SUBSIDY_SCRIPT)
		if type(entity) ~= "number" or entity < 0 then return nil end
		local c = api.engine.getComponent(entity, api.type.ComponentType.GAME_SCRIPT)
		return c and c.state
	end)
	if ok and type(state) == "table" then return state end
	return nil
end

-- Where the subsidy `uid` is in the script's `state`: "offered", "taken",
-- "completed" or "failed", and the subsidy, the first the script would find
-- under that number; and how many offers share the number. Nil when it is
-- in none.
function companies.findSubsidy(state, uid)
	if type(state) ~= "table" or type(uid) ~= "number" then return nil end
	local found, where, offers = nil, nil, 0
	for _, list in ipairs(SUBSIDY_LISTS) do
		for _, s in ipairs(type(state[list[1]]) == "table" and state[list[1]] or {}) do
			if type(s) == "table" and s.uid == uid then
				if list[2] == "offered" then offers = offers + 1 end
				if found == nil then found, where = s, list[2] end
			end
		end
	end
	return where, found, offers
end

-- The money a subsidy's bonuses or penalties book (`list`, the script's
-- SubventionBonusMalus list): the sum of the Money ones' amounts, as the
-- script floors them.
function companies.subsidyMoney(list)
	local sum = 0
	for _, b in ipairs(type(list) == "table" and list or {}) do
		local amount = type(b) == "table" and b.type == "Money" and type(b.params) == "table" and b.params.amount
		if type(amount) == "number" then sum = sum + math.floor(amount) end
	end
	return sum
end

-- The record of subsidy `uid` of kind `kind` the room keeps, and its index.
local function subsidyRecord(roster, uid, kind)
	for i, r in ipairs(roster.subsidies or {}) do
		if r.uid == uid and r.kind == kind then return r, i end
	end
	return nil
end

-- Moves `amount` of subsidy money the script booked to the first company on
-- to company `c` (a negative amount, a penalty, back from it).
local function moveSubsidy(roster, c, amount, send, api)
	local first = companies.find(roster, 0)
	if amount == 0 or not c or c.id == 0 or not first then return end
	book(api, send, first.entity, -amount, "SUBSIDY")
	book(api, send, c.entity, amount, "SUBSIDY")
end

-- Checks that the offer `ref` ({ uid, kind }) can be answered: returns the
-- subsidy, or nil and why, the same in every game.
local function offered(roster, ref, state)
	if type(ref) ~= "table" or type(ref.uid) ~= "number" or type(ref.kind) ~= "string" then
		return nil, "a subsidy by its number and kind"
	end
	if state == nil then return nil, "this game has no subsidy script" end
	local where, s, offers = companies.findSubsidy(state, ref.uid)
	if s == nil then return nil, "the subsidy is no longer offered" end
	if s.id ~= ref.kind then return nil, "the subsidy under that number is another one" end
	if where ~= "offered" then
		local r = subsidyRecord(roster, ref.uid, ref.kind)
		local taker = r and companies.find(roster, r.company)
		if where == "taken" then
			return nil, "the subsidy was taken already, by " .. (taker and taker.name or "the room's first company")
		end
		return nil, "the subsidy is " .. where .. " already"
	end
	if offers > 1 then return nil, "two offers share that number" end
	return s
end

-- Company `id` takes the subsidy offer `ref`: `sendEvent()` sends the
-- script's own onAccept, which runs at once and books the money up front
-- to the first company; for another company, every game moves it on.
-- Returns true, or false and why; nothing changes when it returns false.
function companies.acceptSubsidy(roster, id, ref, state, sendEvent, send, api)
	local c = companies.find(roster, id)
	if not c or c.gone then return false, "there is no such company" end
	local s, why = offered(roster, ref, state)
	if not s then return false, why end
	local upfront = companies.subsidyMoney(type(s.data) == "table" and s.data.upfront)
	sendEvent()
	moveSubsidy(roster, c, upfront, send, api)
	roster.subsidies = roster.subsidies or {}
	local kept = {}
	for _, r in ipairs(roster.subsidies) do
		if not (r.uid == ref.uid and r.kind == ref.kind) then kept[#kept + 1] = r end
	end
	kept[#kept + 1] = { uid = ref.uid, kind = ref.kind, company = id, state = "taken" }
	roster.subsidies = kept
	return true
end

-- Declines the subsidy offer `ref`, for every company: `sendEvent()` sends
-- the script's own onDecline. Returns true, or false and why.
function companies.declineSubsidy(roster, ref, state, sendEvent)
	local s, why = offered(roster, ref, state)
	if not s then return false, why end
	sendEvent()
	return true
end

-- The game day now, counted from the game's start; nil where the game does
-- not say.
function companies.dayNow(api)
	local ok, day = pcall(function()
		local gt = api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME)
		local length = api.util.getDefaultDayDuration()
		if type(length) ~= "number" or length <= 0 or type(gt.gameTime) ~= "number" then return nil end
		return math.floor(gt.gameTime / length)
	end)
	if ok then return day end
	return nil
end

-- Whether another company's subsidies are to be settled: one it took is
-- still open, and a game day has begun since the last settled.
function companies.subsidiesDue(roster, day)
	if type(roster) ~= "table" or type(day) ~= "number" then return false end
	if roster.subsidyDay ~= nil and day <= roster.subsidyDay then return false end
	for _, r in ipairs(roster.subsidies or {}) do
		if r.company ~= 0 and r.state == "taken" then return true end
	end
	return false
end

-- Settles the subsidies the room keeps against the script's `state`: a
-- subsidy another company took that the script completed has its reward
-- moved on to that company, one that failed its penalty; one the script no
-- longer has, or one settled for good, is forgotten. A taker that is gone
-- (dissolved) gets nothing and pays nothing: the first company gives back
-- the reward, or gets back the penalty, the script booked to it, so it
-- ends with nothing of a subsidy it did not take. Returns what it did, as
-- lines for the log.
function companies.settleSubsidies(roster, state, day, send, api)
	if not acceptance.subsidies then return end
	local said, kept = {}, {}
	if type(state) ~= "table" then return said end
	roster.subsidyDay = day
	for _, r in ipairs(roster.subsidies or {}) do
		local where, s = companies.findSubsidy(state, r.uid)
		local c = companies.find(roster, r.company)
		local keep = s ~= nil and s.id == r.kind and where ~= "failed"
		if s ~= nil and s.id == r.kind and c and c.id ~= 0 and r.state == "taken" then
			local data = type(s.data) == "table" and s.data or {}
			local first = companies.find(roster, 0)
			if where == "completed" then
				local amount = companies.subsidyMoney(data.complete)
				r.state = "completed"
				if not c.gone then
					moveSubsidy(roster, c, amount, send, api)
					said[#said + 1] = "subsidy " .. string.format("%d", r.uid) .. " completed for " .. c.name .. ": " .. amount
				elseif first and amount ~= 0 then
					-- Its taker is gone: the reward is no one's, and the
					-- first company gives back what the script booked it.
					book(api, send, first.entity, -amount, "SUBSIDY")
					said[#said + 1] = "subsidy " .. string.format("%d", r.uid) .. " completed for " .. c.name
						.. ", gone: its reward of " .. amount .. " is no one's"
				end
			elseif where == "failed" then
				local amount = companies.subsidyMoney(data.failure)
				if not c.gone then
					moveSubsidy(roster, c, -amount, send, api)
					said[#said + 1] = "subsidy " .. string.format("%d", r.uid) .. " failed for " .. c.name .. ": -" .. amount
				elseif first and amount ~= 0 then
					-- Its taker is gone: no one pays its penalty, and the
					-- first company gets back what the script charged it.
					book(api, send, first.entity, amount, "SUBSIDY")
					said[#said + 1] = "subsidy " .. string.format("%d", r.uid) .. " failed for " .. c.name
						.. ", gone: its penalty of " .. amount .. " is no one's"
				end
			end
		end
		-- Completed, it stays the company's in the script for years: kept
		-- so a second accept names who took it.
		if keep then kept[#kept + 1] = r end
	end
	roster.subsidies = kept
	return said
end

-- Applies one `CompanyOp` for `player`. `send(command)` runs a command at
-- once and returns its data and result entities. `seal` is the room's seal
-- of the password sent with it, { scope =, tag = }, or nil. Returns true, or
-- false and why; the roster changes only when it returns true. A reason
-- never says more of a password than whether it fitted.
function companies.run(roster, player, op, send, api, seal)
	if type(op) ~= "table" then return false, "a company operation is a table" end
	local kind, body = next(op)
	if kind == "Create" then
		local name = trimmed(type(body) == "table" and body.name)
		if name == nil then return false, "a company needs a name" end
		if #companies.live(roster) >= companies.MAX then
			return false, "the room has " .. companies.MAX .. " companies already"
		end
		if nameTaken(roster, name) then return false, "a company is called " .. name .. " already" end
		local color = freeColor(roster)
		local data, entities = send(api.cmd.makeGameAddPlayerCmd(name, vec(api, color)))
		local entity = made(data, entities, "resultEntity")
		if entity == nil then return false, "the game made no company" end
		local id = roster.next
		roster.next = id + 1
		roster.list[#roster.list + 1] = { id = id, entity = entity, name = name, color = color, founder = player }
		setMember(roster, player, id)
		companies.ensureLoanOffers(roster, api)
		return true, nil, id
	elseif kind == "Join" then
		local c = companies.find(roster, body)
		if c == nil or c.gone then return false, "there is no company " .. tostring(body) end
		if memberOf(roster, player, c.id) then return true, nil, c.id end
		if companies.locked(c) then
			if not sealFor(seal, c.id) then return false, "joining " .. c.name .. " needs its password" end
			if seal.tag ~= c.lock.tag then return false, "the password for " .. c.name .. " is not right" end
		end
		if c.id == 0 then leave(roster, player) else setMember(roster, player, c.id) end
		return true, nil, c.id
	elseif kind == "Rename" then
		local c = type(body) == "table" and companies.find(roster, body.company)
		if not c or c.gone then return false, "there is no such company" end
		if not memberOf(roster, player, c.id) then return false, "only its players rename a company" end
		local name = trimmed(body.name)
		if name == nil then return false, "a company needs a name" end
		if nameTaken(roster, name, c.id) then return false, "a company is called " .. name .. " already" end
		send(api.cmd.makeEntitySetNameCmd(c.entity, name))
		c.name = name
		return true, nil, c.id
	elseif kind == "Recolor" then
		local c = type(body) == "table" and companies.find(roster, body.company)
		if not c or c.gone then return false, "there is no such company" end
		if not memberOf(roster, player, c.id) then return false, "only its players recolour a company" end
		local color = colorOf(body.color)
		if not color then return false, "a colour is { r, g, b }, each from 0 to 1" end
		for _, other in ipairs(companies.live(roster)) do
			if other.id ~= c.id and sameColor(other.color, color) then
				return false, other.name .. " wears that colour already"
			end
		end
		c.color = color
		companies.paintFleet(c, send, api)
		return true, nil, c.id
	elseif kind == "Delete" then
		-- Its last player dissolves it, when it owns nothing, and plays for
		-- the room's first company again (tpf3mp_testkit's regression model
		-- has the same rule).
		local c = companies.find(roster, body)
		if not c or c.gone then return false, "there is no company " .. tostring(body) end
		if c.id == 0 then return false, "the room's first company stays" end
		if not memberOf(roster, player, c.id) then return false, "only its players dissolve a company" end
		if #companies.members(roster, c.id) > 1 then return false, "others still play for " .. c.name end
		local owns = companies.owns(api, c.entity)
		if owns == nil then return false, "this game cannot tell what " .. c.name .. " owns" end
		if owns then return false, c.name .. " still owns something" end
		c.gone = true
		leave(roster, player)
		passwordResolved(roster, c.id)
		return true, nil, c.id
	elseif kind == "Lock" then
		-- Its head gives it a password, or a new one: every game keeps the
		-- room's seal of it, never the password.
		local c = companies.find(roster, body)
		if not c or c.gone then return false, "there is no company " .. tostring(body) end
		local ok, why = headOf(roster, player, c, "gives a password to")
		if not ok then return false, why end
		if not sealFor(seal, c.id) then return false, "a password for " .. c.name .. " comes sealed by the room" end
		c.lock = { scope = seal.scope, tag = seal.tag }
		passwordResolved(roster, c.id)
		return true, nil, c.id
	elseif kind == "Unlock" then
		local c = companies.find(roster, body)
		if not c or c.gone then return false, "there is no company " .. tostring(body) end
		local ok, why = headOf(roster, player, c, "takes the password from")
		if not ok then return false, why end
		c.lock = nil
		passwordResolved(roster, c.id)
		return true, nil, c.id
	elseif kind == "Dismiss" then
		-- Its head sends a player out: they play for the room's first
		-- company again. What they built stays the company's.
		local c = type(body) == "table" and companies.find(roster, body.company)
		if not c or c.gone then return false, "there is no such company" end
		local ok, why = headOf(roster, player, c, "sends players out of")
		if not ok then return false, why end
		if body.player == player then return false, "the head leaves by joining another company" end
		if not memberOf(roster, body.player, c.id) then return false, "that player does not play for " .. c.name end
		leave(roster, body.player)
		return true, nil, c.id
	elseif kind == "ShareStations" then
		local c = type(body) == "table" and companies.find(roster, body.company)
		if not c or c.gone then return false, "there is no such company" end
		if type(body.open) ~= "boolean" then return false, "stations are open or not" end
		local ok, why = headOf(roster, player, c, body.open and "opens the stations of" or "closes the stations of")
		if not ok then return false, why end
		c.closed = (not body.open) or nil
		return true, nil, c.id
	elseif kind == "StationAccess" then
		-- Its head's choice for one other company, over the default; nil
		-- leaves that company to the default again.
		local c = type(body) == "table" and companies.find(roster, body.company)
		if not c or c.gone then return false, "there is no such company" end
		if body.open ~= nil and type(body.open) ~= "boolean" then return false, "stations are open or not" end
		local ok, why = headOf(roster, player, c, "decides whose lines stop at the stations of")
		if not ok then return false, why end
		local other = companies.find(roster, body.other)
		if not other or other.gone then return false, "there is no company " .. tostring(body.other) end
		if other.id == c.id then return false, c.name .. "'s stations are always its own" end
		local kept = {}
		for _, a in ipairs(c.access or {}) do
			if a.company ~= other.id then kept[#kept + 1] = a end
		end
		if body.open ~= nil then kept[#kept + 1] = { company = other.id, open = body.open } end
		c.access = #kept > 0 and kept or nil
		return true, nil, c.id
	end
	return false, "a company operation of no kind"
end

return companies
