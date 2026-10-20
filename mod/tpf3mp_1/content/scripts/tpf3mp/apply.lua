-- tpf3mp/apply.lua -- runs an action the room ordered, in the mod's game
-- script's postUpdate (docs/HOOKS.md, "Actions in the game").
--
-- A game script runs in an engine state, where a command runs at once (the
-- game's own api/tealdef/api/cmd.d.tl). The game takes no callback in
-- update ("Callbacks are currently disallowed", build 40408), but in
-- postUpdate, where this runs, it calls one at once, with the command's
-- result: the game's mission scripts read the line they made from it right
-- after sendCommand (mission_vehicle_util.tl). A command the game refuses
-- raises, or its callback hears that it failed.
-- Every game applies the room's action in the same simulation update, so
-- what this makes of an action may depend on nothing but the action and the
-- world, which every game has alike: no time of day, no camera, no GUI.
--
-- An action is the table the hook hands over (tpf3mp_proto::lua): one
-- entry, the action's name and its body, in the game's units (metres,
-- plain fractions).
--
-- Pure Lua against the game's `api`; the tests give it a fake one.

local acceptance = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/acceptance.lua")
    or require("tpf3mp.acceptance")

local apply = {}

-- A matrix from a Transform: its basis is elements 1-3, 5-7 and 9-11 of the
-- game's matrix and its origin elements 13-15 (columns of four).
local function matrix(transform)
	local b, o = transform.basis, transform.origin
	local column = api.type.Vec4f.new
	return api.type.Mat4f.new(
		column(b[1], b[2], b[3], 0),
		column(b[4], b[5], b[6], 0),
		column(b[7], b[8], b[9], 0),
		column(o.x, o.y, o.z, 1)
	)
end

-- A parameter's value: exactly one of Int, Fixed, Bool and Text.
local function paramValue(value)
	if value.Int ~= nil then return value.Int end
	if value.Fixed ~= nil then return value.Fixed end
	if value.Bool ~= nil then return value.Bool end
	if value.Text ~= nil then return value.Text end
	error("a parameter value of no kind")
end

-- The keys a flattened parameter path names: "modules[3801].name" is
-- modules, 3801, name.
local function pathKeys(path)
	local keys = {}
	for part in string.gmatch(path, "[^%.]+") do
		local name, rest = string.match(part, "^([^%[]*)(.*)$")
		if name ~= "" then keys[#keys + 1] = name end
		for index in string.gmatch(rest, "%[(%-?%d+)%]") do
			keys[#keys + 1] = tonumber(index)
		end
	end
	if #keys == 0 then error("an empty parameter path") end
	return keys
end

-- The construction's parameters, from their flattened paths.
local function params(list)
	local out = {}
	for _, param in ipairs(list) do
		local keys = pathKeys(param.key)
		local node = out
		for i = 1, #keys - 1 do
			local key = keys[i]
			if type(node[key]) ~= "table" then node[key] = {} end
			node = node[key]
		end
		node[keys[#keys]] = paramValue(param.value)
	end
	return out
end

-- A stop's or signal's parameters, from engine.flatParams' list: flat keys,
-- each taken whole. A mod's key may hold a dot (Signal Distance's
-- "signal_distance.local"), which is no path here.
local function objectParams(list)
	local out = {}
	for _, param in ipairs(list) do out[param.key] = paramValue(param.value) end
	return out
end

-- Whether a handler runs dry (apply.proposalOf): it makes the proposal its
-- action would build and stops there, sending nothing and saying nothing.
local dry = false

-- A line for the hook's log; the game script sets apply.log once linked.
local function log(line)
	if dry then return end
	if apply.log then pcall(apply.log, line) end
end

-- A list of the action, afresh, for the game to copy. The game copies a
-- list it is handed into its own vector in the order `next` walks it, and
-- the actions reach postUpdate as the game's own copy of what update
-- returned, whose lists `next` walks in hash order (build 40408: a bus
-- line's stops set to load grain, one cargo over from passengers). A table
-- filled 1, 2, 3... walks in order.
local function seq(list)
	local out = {}
	for i = 1, #list do out[i] = list[i] end
	return out
end

-- What the action running now costs, by part, for the log only (apply.run,
-- apply.proposalOf): os.clock around the engine's calls that can take long
-- in a big world, the elapsed time on Windows and the process's CPU time on
-- Linux and macOS (the C library's clock). Nothing read from it reaches
-- the world. Each part is
-- { n = calls, s = seconds, items = what the calls went through }, in
-- `order` by first use; nil while no action runs.
local costs = nil

local function clock()
	local ok, t = pcall(function() return os.clock() end)
	if ok and type(t) == "number" then return t end
	return nil
end

local function spend(part, t0, items)
	if costs == nil or t0 == nil then return end
	local t1 = clock()
	if t1 == nil then return end
	local c = costs.parts[part]
	if c == nil then
		c = { n = 0, s = 0, items = 0 }
		costs.parts[part] = c
		costs.order[#costs.order + 1] = part
	end
	c.n, c.s, c.items = c.n + 1, c.s + (t1 - t0), c.items + (items or 0)
end

-- The parts of a cost as one line: "nodes Street 1x 180.2 ms (41022), ...".
function apply.costText(cost)
	if type(cost) ~= "table" or type(cost.order) ~= "table" then return "" end
	local out = {}
	for _, part in ipairs(cost.order) do
		local c = cost.parts[part]
		out[#out + 1] = string.format("%s %dx %.1f ms", part, c.n, c.s * 1000)
			.. (c.items > 0 and (" (" .. c.items .. ")") or "")
	end
	return table.concat(out, ", ")
end

-- The last action's cost (apply.run, apply.proposalOf): { kind =, ms =,
-- parts =, order = }, or nil where the clock cannot say.
apply.lastCost = nil
-- Actions that took at least this long are said in the log, with their parts.
apply.SLOW_MS = 20

local function costStart(kind)
	costs = { kind = kind, parts = {}, order = {}, t0 = clock() }
	apply.lastCost = nil
end

local function costEnd()
	local c = costs
	costs = nil
	local t1 = clock()
	if c == nil or c.t0 == nil or t1 == nil then return nil end
	c.ms = (t1 - c.t0) * 1000
	apply.lastCost = c
	return c
end

-- Whether this Lua state's game takes a command's callback here; the game
-- runs game scripts on a pool of states, so each learns on its own.
local callbacks = true

-- Sends `command`, which runs at once, and returns what the game answered,
-- its command data and result entities (nil, where it answers nothing
-- here). A command the game refuses raises, and apply.run reports it.
local function sendNow(command)
	if callbacks then
		local heard, went, data, entities = false, nil, nil, nil
		local sent, err = pcall(api.cmd.sendCommand, command, function(d, success, e)
			heard, went, data, entities = true, success, d, e
		end)
		if sent then
			if heard and went ~= true then error("the game refused it", 0) end
			return data, entities
		end
		-- The game refuses a callback before it runs anything: sent again
		-- without one, the command runs once.
		if not tostring(err):find("allbacks are currently disallowed", 1, true) then error(err, 0) end
		callbacks = false
		log("the game takes no command callbacks in this state: what an action makes is found by the registry alone")
	end
	api.cmd.sendCommand(command)
	return nil, nil
end

local function send(command)
	if dry then error({ dry = "a command that is not a build" }, 0) end
	local t0 = costs and clock()
	local data, entities = sendNow(command)
	spend("commands", t0)
	return data, entities
end

local function run(command)
	send(command)
	return true
end

-- For the game script's own commands (tpf3mp/companies.lua's monthly loan
-- payments): the same send, which answers what the game made.
apply.send = send

local require_companies

-- The action running now: `ctx` as apply.run was given it. Its `company` is
-- the player entity of the acting player's company (tpf3mp/companies.lua);
-- without one, the save's own player, as before companies.
local acting = nil

local function company()
	return (acting and acting.company) or api.engine.util.getPlayer()
end

-- Refuses changing `entity` when another company owns it, naming the owner,
-- the same in every game (tpf3mp/companies.lua).
local function mine(entity, what)
	local companies = require_companies()
	local ok, why = companies.mayTouch(acting and acting.roster, company(), entity, api, what)
	if not ok then error(why, 0) end
end

-- Change-time diagnostics read only a handful of components for the action
-- being replayed. These reads are optional: a missing component is `none`,
-- and a getter which the game cannot read is `unavailable`. Diagnostic
-- failures never decide whether the ordered action applies.
local function diagnosticField(value, key)
	if value == nil then return nil, "none" end
	local ok, result = pcall(function() return value[key] end)
	if not ok then return nil, "unavailable" end
	if result == nil then return nil, "none" end
	return result, "value"
end

local function diagnosticComponent(entity, name)
	if type(entity) ~= "number" or entity < 0 then return nil, "none" end
	local okType, componentType = pcall(function()
		local types = api.type and api.type.ComponentType
		return types and types[name]
	end)
	if not okType or componentType == nil then return nil, "unavailable" end
	local ok, component = pcall(function() return api.engine.getComponent(entity, componentType) end)
	if not ok then return nil, "unavailable" end
	if component == nil then return nil, "none" end
	return component, "value"
end

local function diagnosticToken(value)
	local text = type(value) == "string" and value or tostring(value)
	return (text:gsub("[^%w_./:%-]", "?"):sub(1, 128))
end

local function diagnosticEntity(value)
	if type(value) ~= "number" or value < 0 or value ~= value or value == math.huge or value == -math.huge then
		return "unavailable"
	end
	return string.format("%.0f", value)
end

local function diagnosticCompany(roster, entity)
	if type(entity) ~= "number" or entity < 0 then return "none" end
	local ok, companies = pcall(require_companies)
	if not ok or type(companies) ~= "table" or type(companies.byEntity) ~= "function" then
		return "unavailable"
	end
	local read, c = pcall(companies.byEntity, roster, entity)
	if not read then return "unavailable" end
	if c == nil then return "unmapped/entity-" .. diagnosticEntity(entity) end
	return "company-" .. diagnosticEntity(c.id) .. "/entity-" .. diagnosticEntity(entity)
end

local function diagnosticActor(roster, player)
	if player == nil then return "none" end
	local ok, companies = pcall(require_companies)
	if not ok or type(companies) ~= "table" or type(companies.of) ~= "function" then
		return "unavailable"
	end
	local read, c = pcall(companies.of, roster, player)
	if not read then return "unavailable" end
	if c == nil then return "none" end
	return "company-" .. diagnosticEntity(c.id) .. "/entity-" .. diagnosticEntity(c.entity)
end

local function diagnosticLocal(roster)
	local ok, selected = pcall(function() return api.engine.util.getPlayer() end)
	if not ok then return "unavailable" end
	if selected == nil or (type(selected) == "number" and selected < 0) then return "none" end
	if type(selected) ~= "number" or selected ~= selected or selected == math.huge or selected == -math.huge then
		return "unavailable"
	end
	return diagnosticCompany(roster, selected)
end

local function diagnosticOwner(entity, roster)
	local component, status = diagnosticComponent(entity, "PLAYER_OWNED")
	if status ~= "value" then return status end
	local owner, ownerStatus = diagnosticField(component, "player")
	if ownerStatus == "none" then return "none" end
	if ownerStatus ~= "value" or type(owner) ~= "number" or owner < 0 then return "unavailable" end
	return diagnosticCompany(roster, owner)
end

local function diagnosticActionIndex(ctx)
	local step, status = diagnosticField(ctx, "actionIndex")
	if status ~= "value" or type(step) ~= "number" or step < 0 or step ~= math.floor(step) then return "none" end
	return string.format("%.0f", step)
end

local function diagnosticSignedInteger(value)
	if type(value) ~= "number" or value ~= math.floor(value) or value ~= value
			or value == math.huge or value == -math.huge then
		return "unavailable"
	end
	return string.format("%.0f", value)
end

local function diagnosticSequenceLength(value)
	local ok, length = pcall(function() return #value end)
	if not ok or type(length) ~= "number" or length < 0 or length > 256 then return nil end
	return length
end

local function diagnosticModelName(modelId)
	if type(modelId) ~= "number" or modelId < 0 then return "none" end
	local ok, name = pcall(function() return api.res.modelRep.getName(modelId) end)
	if not ok then return "unavailable" end
	if type(name) ~= "string" or name == "" then return "none" end
	return diagnosticToken(name)
end

local function diagnosticVehicleModels(entity)
	local vehicle, status = diagnosticComponent(entity, "TRANSPORT_VEHICLE")
	if status ~= "value" then return status, status end
	local config, configStatus = diagnosticField(vehicle, "transportVehicleConfig")
	if configStatus ~= "value" then return configStatus, configStatus end
	local parts, partsStatus = diagnosticField(config, "vehicles")
	if partsStatus ~= "value" then return partsStatus, partsStatus end
	local length = diagnosticSequenceLength(parts)
	if length == nil then return "unavailable", "unavailable" end
	if length == 0 then return "none", "none" end
	local names, carriers, failed = {}, {}, false
	local count = math.min(length, 8)
	for i = 1, count do
		local entry, entryStatus = diagnosticField(parts, i)
		local part, partStatus = diagnosticField(entry, "part")
		local modelId, modelStatus = diagnosticField(part, "modelId")
		local name = "unavailable"
		if entryStatus == "value" and partStatus == "value" and modelStatus == "value" then
			name = diagnosticModelName(modelId)
		elseif entryStatus == "value" and partStatus == "value" and modelStatus == "none" then
			name = "none"
		end
		names[#names + 1] = name
		if name == "unavailable" then
			failed = true
		else
			local carrier = name:match("^vehicle/([%w_-]+)/")
			if carrier then carriers[carrier] = true end
		end
	end
	if length > count then names[#names + 1] = "+" .. tostring(length - count) end
	local carrierNames = {}
	for carrier in pairs(carriers) do carrierNames[#carrierNames + 1] = carrier end
	table.sort(carrierNames)
	local carrier = "none"
	if failed then
		carrier = "unavailable"
	elseif #carrierNames == 1 then
		carrier = diagnosticToken(carrierNames[1])
	elseif #carrierNames > 1 then
		carrier = "mixed"
	end
	return carrier, table.concat(names, ",")
end

local function diagnosticLineModes(entity)
	local line, status = diagnosticComponent(entity, "LINE")
	if status ~= "value" then return status end
	local info, infoStatus = diagnosticField(line, "vehicleInfo")
	if infoStatus ~= "value" then return infoStatus end
	local modes, modesStatus = diagnosticField(info, "transportModes")
	if modesStatus ~= "value" then return modesStatus end
	local selected = {}
	for mode = 0, 15 do
		local ok, on = pcall(function() return modes[mode] end)
		if not ok then return "unavailable" end
		if on == true then selected[#selected + 1] = tostring(mode) end
	end
	return #selected > 0 and table.concat(selected, ",") or "none"
end

local function diagnosticVehicleAssignment(ctx, vehicleId, vehicle, lineId, line, first, outcome)
	local carrier, models = "unavailable", "unavailable"
	local read, foundCarrier, foundModels = pcall(diagnosticVehicleModels, vehicle)
	if read then carrier, models = foundCarrier, foundModels end
	local mode = "unavailable"
	local modeRead, foundMode = pcall(diagnosticLineModes, line)
	if modeRead then mode = foundMode end
	local owner = "unavailable"
	local ownerRead, foundOwner = pcall(diagnosticOwner, vehicle, ctx and ctx.roster)
	if ownerRead then owner = foundOwner end
	local lineOwner = "unavailable"
	local lineOwnerRead, foundLineOwner = pcall(diagnosticOwner, line, ctx and ctx.roster)
	if lineOwnerRead then lineOwner = foundLineOwner end
	log("assign-vehicle action_index=" .. diagnosticActionIndex(ctx)
		.. " vehicle=" .. diagnosticEntity(vehicleId) .. " native="
		.. (vehicle == nil and "none" or diagnosticEntity(vehicle))
		.. " line=" .. diagnosticEntity(lineId) .. " native_line="
		.. (line == nil and "none" or diagnosticEntity(line))
		.. " first=" .. diagnosticSignedInteger(first) .. " carrier=" .. carrier .. " models=" .. models
		.. " mode=" .. mode .. " owner=" .. owner .. " line_owner=" .. lineOwner
		.. " outcome=" .. outcome)
end

local function diagnosticConstruction(ref, entity, component, roster)
	local file, fileStatus = diagnosticField(ref, "file")
	local at, atStatus = diagnosticField(ref, "at")
	local x, xs = diagnosticField(at, "x")
	local y, ys = diagnosticField(at, "y")
	local z, zs = diagnosticField(at, "z")
	local function point(a, as, b, bs, c, cs)
		if as ~= "value" or bs ~= "value" or cs ~= "value" or type(a) ~= "number" or type(b) ~= "number"
				or type(c) ~= "number" or a ~= a or b ~= b or c ~= c or a == math.huge or b == math.huge
				or c == math.huge or a == -math.huge or b == -math.huge or c == -math.huge then
			return "unavailable"
		end
		return string.format("%.1f,%.1f,%.1f", a, b, c)
	end
	local requested = fileStatus == "value" and diagnosticToken(file) or fileStatus
	local transf, transfStatus = diagnosticField(component, "transf")
	local ax, axs = diagnosticField(transf, 13)
	local ay, ays = diagnosticField(transf, 14)
	local az, azs = diagnosticField(transf, 15)
	local actual = transfStatus == "value" and point(ax, axs, ay, ays, az, azs) or transfStatus
	local refPoint = atStatus == "value" and point(x, xs, y, ys, z, zs) or atStatus
	local owner = "unavailable"
	local ownerRead, foundOwner = pcall(diagnosticOwner, entity, roster)
	if ownerRead then owner = foundOwner end
	log("bulldoze-construction action_index=" .. diagnosticActionIndex(acting)
		.. " ref=" .. requested .. "@(" .. refPoint .. ") native=" .. diagnosticEntity(entity)
		.. " actual=(" .. actual .. ") owner=" .. owner)
end

-- The entity a command made: its data's field `field`, else the first of
-- its result entities; nil when the game did not say.
local function madeBy(field, data, entities)
	local ok, e = pcall(function() return data[field] end)
	if ok and type(e) == "number" and e >= 0 then return e end
	local first = type(entities) == "table" and entities[1]
	e = type(first) == "table" and first[1] or nil
	if type(e) == "number" and e >= 0 then return e end
	return nil
end

-- The game's verdict on `proposal`: the reasons it refuses it, or nil when
-- it would build it (and its warnings logged).
local function refusal(proposal, context)
	local proposals = api.engine.util.proposal
	if not (proposals and proposals.makeProposalData) then return nil end
	local t0 = costs and clock()
	local data = proposals.makeProposalData(proposal, context)
	spend("verdict", t0)
	local state = data and data.errorState
	local messages = {}
	for _, m in ipairs(state and state.messages or {}) do messages[#messages + 1] = tostring(m) end
	if state and state.critical then return "the game refuses the build: " .. table.concat(messages, "; ") end
	if #messages > 0 then log("the game warns of the build: " .. table.concat(messages, "; ")) end
	return nil
end

-- Builds `proposal` as the player's own build. The game's verdict first, as
-- its tools ask it: a build it would refuse (a collision, too steep, not
-- enough money) fails here with its reasons, the same in every game, and is
-- never sent; sent without a callback, a refused build would fail unseen.
local function buildProposal(proposal, context)
	-- Dry: the proposal is what was asked for; the game's verdict and the
	-- build are not.
	if dry then error({ dry = true, proposal = proposal, context = context }, 0) end
	local refused = refusal(proposal, context)
	if refused then error(refused, 0) end
	-- What is not critical the tool builds through once the player clicks,
	-- town buildings in the way included: ignoreErrors, as the player's own
	-- build (with it false the game drops such a build unseen).
	return run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
end

local HANDLERS = {}

-- Fills a SimpleProposal's street proposal from a polyline ("roads", below).
local networkInto
-- Builds a construction whose own tracks end on existing track ends
-- ("roads", below).
local rejoinConstruction
-- The construction of a file at a place ("vehicles and lines", below).
local constructionAt
-- Removes a stop from its edge ("stops", below).
local removeEdgeObject

-- Names station groups the room built, the same in every game (2026-10-02:
-- room-built stops and stations stood unnamed). The game's own tools name
-- them natively; the room carries that name from the originator's proposal.
-- A script build with an empty name leaves its entities with no NAME (docs/BUILDING.md,
-- "What a script proposal must carry"). `groups` lists { group =, stations
-- = { ... } }, each a group this build's own stations alone make up; one
-- with a name of its own already keeps it. `name` is the name to give, or
-- nil for the town's: the town the game counts the group's first station
-- in (stationSystem.getTown), and a number after it where another station
-- group of that town has that name ("Didcot", "Didcot 2", ...), so two
-- games, which hold the same world, give the same. `fallback` where the
-- station has no town. Each group and its stations get the name
-- (makeEntitySetNameCmd); hook.log says which.
local PROVISIONAL_STOP_NAME = "Stop"
-- Whether a stop is built with the name the originator's tool gave it
-- (PlaceStop.name, the game's own: street_util::MakeEdgeObjectName). The
-- kill switch: false, and every stop is named by the town as above.
apply.NATIVE_STOP_NAMES = true
local function nameStationGroups(groups, name, fallback)
	local named = {}
	for _, g in ipairs(groups) do
		local current
		pcall(function() current = api.engine.util.getEntityName(g.group) end)
		if type(current) ~= "string" or current == "" or current == PROVISIONAL_STOP_NAME then
			local chosen = name
			if chosen == nil then
				local town = -1
				pcall(function() town = api.engine.system.stationSystem.getTown(g.stations[1]) end)
				local townName
				if type(town) == "number" and town >= 0 then
					pcall(function() townName = api.engine.util.getEntityName(town) end)
				end
				if type(townName) == "string" and townName ~= "" then
					-- The names the town's other station groups have.
					local taken = {}
					pcall(function()
						for _, s in ipairs(api.engine.system.stationSystem.getStations(town) or {}) do
							local other = api.engine.system.stationGroupSystem.getStationGroup(s)
							if type(other) == "number" and other >= 0 and other ~= g.group then
								local n = api.engine.util.getEntityName(other)
								if type(n) == "string" then taken[n] = true end
							end
						end
					end)
					chosen = townName
					local k = 2
					while taken[chosen] do
						chosen = townName .. " " .. k
						k = k + 1
					end
				else
					chosen = fallback
				end
			end
			if type(chosen) == "string" and chosen ~= "" then
				local ok, why = pcall(function()
					send(api.cmd.makeEntitySetNameCmd(g.group, chosen))
					for _, s in ipairs(g.stations) do send(api.cmd.makeEntitySetNameCmd(s, chosen)) end
				end)
				named[#named + 1] = "station group " .. g.group .. " \"" .. chosen .. "\""
					.. (ok and "" or (": refused, " .. tostring(why)))
			end
		end
	end
	if #named > 0 then log("named " .. table.concat(named, ", ")) end
end

-- The station groups construction `con`'s stations alone make up, as
-- nameStationGroups takes them.
local function constructionGroups(con, mustResolve)
	local c = api.engine.getComponent(con, api.type.ComponentType.CONSTRUCTION)
	local mine, byGroup, out = {}, {}, {}
	for _, s in ipairs(seq(c and c.stations or {})) do mine[s] = true end
	for _, s in ipairs(seq(c and c.stations or {})) do
		local group = -1
		local found, result = pcall(api.engine.system.stationGroupSystem.getStationGroup, s)
		if found then
			group = result
		elseif mustResolve then
			error("cannot find station group for station " .. tostring(s) .. ": " .. tostring(result), 0)
		end
		if type(group) == "number" and group >= 0 and not byGroup[group] then
			local g = api.engine.getComponent(group, api.type.ComponentType.STATION_GROUP)
			if mustResolve and g == nil then error("no station group component on " .. tostring(group), 0) end
			local alone = g ~= nil
			for _, other in ipairs(seq(g and g.stations or {})) do
				if not mine[other] then alone = false end
			end
			if alone then
				byGroup[group] = { group = group, stations = {} }
				out[#out + 1] = byGroup[group]
			end
		end
		if byGroup[group] then table.insert(byGroup[group].stations, s) end
	end
	return out
end

-- The game's refresh of construction `con`, and whether it changes its
-- streets. A scripted build does not snap; the game's refresh of a
-- construction does, as its tool does: the entrance then ends at the street
-- node beside it (refreshConstruction, build 40408: the same edge the tool
-- proposed). Logged as what it snaps. The game's verdict takes simple
-- proposals only ("SimpleProposal expected, got Proposal", build 40408): a
-- refresh the game refuses fails in the command's own answer instead (run).
local function refreshOf(con)
	local t0 = costs and clock()
	local refresh = api.engine.util.proposal.refreshConstruction(con)
	spend("refresh", t0)
	local street, shape = refresh.proposal, {}
	for i = 1, #street.addedSegments do
		local s = street.addedSegments[i]
		shape[#shape + 1] = "+e" .. s.entity .. ":" .. tostring(s.comp.node0) .. ">" .. tostring(s.comp.node1)
	end
	for i = 1, #street.removedSegments do shape[#shape + 1] = "-e" .. tostring(street.removedSegments[i].entity) end
	log("snapping " .. tostring(con) .. " " .. table.concat(shape, " "))
	return refresh, #shape > 0
end

-- A construction the room built is the acting company's, the same in every
-- game, as the game's own missions hand one over
-- (mission_framework_util_entity.tl, setPlayerForConstruction): the
-- construction, its depots, its stations and the station groups they alone
-- make up, and its own (frozen) edges with what stands on them, each
-- given with makeEntitySetPlayerCmd where anyone else owns it, or no one
-- (2026-10-02: a company's depots did not count as its own). The build
-- names the company (`playerEntity`, `Context.player`); this makes sure of
-- what the engine made from it. hook.log names each one handed over.
local function settleConstruction(con, file)
	local C = api.type.ComponentType
	local me = company()
	local seen, fixed = {}, {}
	local function ownerOf(entity)
		local ok, owned = pcall(api.engine.getComponent, entity, C.PLAYER_OWNED)
		if not ok then error("cannot read " .. tostring(entity) .. " owner: " .. tostring(owned), 0) end
		if owned == nil then return nil end
		local read, owner = pcall(function() return owned.player end)
		if not read then error("cannot read " .. tostring(entity) .. " owner: " .. tostring(owner), 0) end
		if owner == nil or (type(owner) == "number" and owner < 0) then return nil end
		if type(owner) ~= "number" then error("invalid owner on " .. tostring(entity), 0) end
		return owner
	end
	local function give(entity, what)
		if type(entity) ~= "number" or entity < 0 then
			error("the game gave no " .. what .. " entity for " .. tostring(file), 0)
		end
		if seen[entity] then return end
		seen[entity] = true
		local owner = ownerOf(entity)
		if owner == me then return end
		send(api.cmd.makeEntitySetPlayerCmd(entity, me))
		fixed[#fixed + 1] = what .. " " .. entity .. " (was " .. tostring(owner) .. ")"
	end
	if type(con) ~= "number" or con < 0 then error("the game made no " .. tostring(file) .. " construction", 0) end
	local c = api.engine.getComponent(con, C.CONSTRUCTION)
	if c == nil then error("no construction component on " .. tostring(con), 0) end
	give(con, "construction")
	for _, depot in ipairs(seq(c.depots or {})) do give(depot, "depot") end
	for _, station in ipairs(seq(c.stations or {})) do give(station, "station") end
	for _, g in ipairs(constructionGroups(con, true)) do give(g.group, "station group") end
	for _, edge in ipairs(seq(c.frozenEdges or {})) do
		give(edge, "edge")
		local e = api.engine.getComponent(edge, C.BASE_EDGE)
		if e == nil then error("no edge component on " .. tostring(edge), 0) end
		for _, o in ipairs(seq(e.objects or {})) do give(o[1], "edge object") end
	end
	if #fixed > 0 then
		log("the new " .. tostring(file) .. " made the acting company's (" .. tostring(me) .. "): "
			.. table.concat(fixed, ", "))
	end
end

-- An edit of a construction (its modules or parameters, an upgrade): the
-- construction the action names removed and the new one built in one
-- proposal, the old mapped to the new (old2new), as the game's own upgrade
-- makes one (mission_framework_util_entity.tl, upgradeConstruction), so
-- what stood on the old one (its stations, their station groups and the
-- lines that stop there) passes to the new. The streets the edit changes
-- around it (a road split for a new exit, 2026-10-03) go in the same
-- proposal, as a new construction's connection does; the old one's own
-- streets go with it, and the connection may not name them. The game's
-- verdict first, and built as the player's own build, paid by the player
-- (buildProposal). The new one stands where the old one stood, so the next
-- edit, a depot or a line finds it by the same file and place.
-- The stock rail editor's own replacement generator preserves its track
-- reconstruction and snapping information. A SimpleProposal only carries
-- the new construction parameters. Regenerate locally, then recapture to
-- ensure the engine has not expanded this into a different requested edit.
-- Reference: build 40408 gui/construction/construction.tl:1295.
local function sameParameter(a, b, tolerance)
	if type(a) ~= type(b) then return false end
	if type(a) == "number" then return a == b or math.abs(a - b) <= tolerance end
	if type(a) ~= "table" then return a == b end
	for k, v in pairs(a) do if not sameParameter(v, b[k], tolerance) then return false end end
	for k in pairs(b) do if a[k] == nil then return false end end
	return true
end

local function railReplacement(build, old, entity)
	local rail = "::/stations/rail/modular_station/modular_station.con"
	if build.file ~= rail or build.replaces.file ~= rail or build.connection ~= nil then return nil end
	local util = api.engine.util and api.engine.util.proposal
	if not util or not util.createProposalReplaceConstruction then
		error("the game cannot regenerate the rail station edit", 0)
	end
	local t0 = costs and clock()
	local full = util.createProposalReplaceConstruction(old, entity.params)
	spend("regenerate", t0)
	if full == nil then error("the game could not regenerate the rail station edit", 0) end
	if #full.toRemove ~= 1 or full.toRemove[1] ~= old or #full.toAdd ~= 1 then
		error("the game regenerated a different rail station edit", 0)
	end
	local capture = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/capture.lua")
		or require("tpf3mp.capture")
	local action, why = capture.construction(full)
	local rebuilt = action and action.BuildConstruction
	if not rebuilt then error("the regenerated rail station edit cannot travel: " .. tostring(why), 0) end
	local same = sameParameter
	if rebuilt.file ~= build.file or rebuilt.name ~= build.name
		or rebuilt.connection ~= nil or not same(rebuilt.replaces, build.replaces, 0.0011)
		or not same(rebuilt.transform.origin, build.transform.origin, 0.0011)
		or not same(rebuilt.transform.basis, build.transform.basis, 0.0000011)
		or not same(params(rebuilt.params), params(build.params), 0.0000011) then
		error("the game regenerated a different rail station edit", 0)
	end
	return full
end

local function replaceConstruction(build, proposal, entity)
	local old, oldComponent = constructionAt(build.replaces)
	mine(old, "construction")
	proposal.constructionsToAdd = { entity }
	proposal.constructionsToRemove = { old }
	proposal.old2new = { [old] = 0 }
	log("replacing " .. tostring(old) .. " " .. tostring(build.replaces.file) .. " with " .. tostring(build.file))
	if build.connection ~= nil then
		local gone = {}
		local frozen = oldComponent and oldComponent.frozenEdges or {}
		for i = 1, #frozen do gone[frozen[i]] = true end
		networkInto(proposal, nil, nil, nil, build.connection, true, gone)
	end
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	context.gatherFields = true
	local native = railReplacement(build, old, entity)
	if native then
		-- Build 40408's makeProposalData binding only accepts SimpleProposal,
		-- despite its Teal declaration. The stock UI sends this full native
		-- proposal directly, with errors enabled. Do not discard its graph
		-- just to pass it through the simple-proposal validator.
		if dry then error({ dry = true, proposal = native, context = context }, 0) end
		run(api.cmd.makeWorldBuildProposalCmd(native, context, false, true))
	else
		buildProposal(proposal, context)
	end
	-- What it made, where the action says: this game could name it.
	local new, component = constructionAt({ file = build.file, at = build.transform.origin })
	if native and not sameParameter(component and component.params, entity.params, 0.0000011) then
		error("the game did not apply the requested rail station parameters", 0)
	end
	-- The acting company's, whatever the engine made of it, as a new one.
	settleConstruction(new, build.file)
	-- The native replacement already includes its snapped track graph.
	if native then return true, new end
	-- The new one makes its entrances again itself, unsnapped, as a build
	-- does: a road station edited by the street came loose from it, its
	-- entrance no longer joined to the junction (2026-10-03, in both games).
	-- So every game refreshes it as a build's, which snaps its entrances,
	-- a new one included, onto the streets beside them as the game's own
	-- edit does. Where nothing is to snap, nothing is sent. The edit stands
	-- in every game either way: a refresh the game refuses leaves it as it
	-- was before, the same everywhere, and is logged.
	local snapped, why = pcall(function()
		local refresh, changes = refreshOf(new)
		if changes then run(api.cmd.makeWorldBuildProposalCmd(refresh, nil, true, false)) end
	end)
	if not snapped then log("the edited construction stays unsnapped: " .. tostring(why)) end
	return true, new
end

function HANDLERS.BuildConstruction(build)
	local proposal = api.type.SimpleProposal.new()
	local entity = api.type.SimpleProposal.ConstructionEntity.new()
	entity.fileName = build.file
	entity.transf = matrix(build.transform)
	entity.params = params(build.params)
	entity.name = build.name
	entity.playerEntity = company()
	if build.replaces ~= nil then return replaceConstruction(build, proposal, entity) end
	-- One headquarters a company (tpf3mp/companies.lua): the game's own
	-- permit counts the whole world's, so every game checks the acting
	-- company's instead. An edit of its headquarters (above) is no second.
	local may, why = require_companies().mayBuild(acting and acting.roster, company(), build.file, api)
	if not may then error(why, 0) end
	proposal.constructionsToAdd = { entity }
	-- The streets the tool built around it, in the same proposal: the
	-- street it joins rebuilt through a junction. Not the construction's own
	-- entrance edge, which the tool snapped onto that junction: the
	-- construction makes its entrance again itself, unsnapped, ending a few
	-- metres short (build 40408).
	if build.connection ~= nil then networkInto(proposal, nil, nil, nil, build.connection, true) end
	-- Paid by the player, and clearing town buildings in its way, as the
	-- construction tool builds (the game's bridge and tunnel window names the
	-- player so, gui/entity_window/bridge_and_tunnel.tl); without a context
	-- the game builds for free. playerInitiated true: as the player's own
	-- build (buildProposal).
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	context.gatherFields = true
	-- The construction alone, as the tool's street part leaves it; where the
	-- game refuses that and the tool joined its own tracks onto track ends
	-- that exist, joined to them as the tool joined it (rejoinConstruction).
	local built
	if dry or build.connection == nil then
		built = buildProposal(proposal, context)
	else
		local refused = refusal(proposal, context)
		if refused then
			-- The fallback removes existing track before a separate re-lay
			-- command. Until an atomic or guaranteed recovery path is shown,
			-- refuse before sending either command. This mod-local gate is part
			-- of the room's fingerprint, never a per-player setting.
			if acceptance.rejoinConstruction ~= true then
				error(refused .. " (snapped track-end rejoin awaits a safe build)", 0)
			end
			built = rejoinConstruction(build, entity, context, refused)
		else
			built = run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
		end
	end
	-- A station's group by the name the tool gave the construction, where
	-- the game left it unnamed (nameStationGroups).
	pcall(function()
		local con = constructionAt({ file = build.file, at = build.transform.origin })
		local groups = constructionGroups(con)
		if #groups > 0 then nameStationGroups(groups, build.name) end
	end)
	-- The acting company's, whatever the engine made of it.
	do
		local con = constructionAt({ file = build.file, at = build.transform.origin })
		settleConstruction(con, build.file)
	end
	if require_companies().isHeadquarters(api, build.file) then
		-- Whether the engine took it as the company's headquarters (its
		-- PLAYER component's `headquarters`), for hook.log: the game's
		-- capital town and its company views read it.
		pcall(function()
			local p = api.engine.getComponent(company(), api.type.ComponentType.PLAYER)
			log("headquarters for company entity " .. tostring(company()) .. ": its PLAYER names "
				.. tostring(p and p.headquarters))
		end)
	end
	if build.connection == nil then return built end
	-- A scripted build does not snap; the game's refresh of a construction
	-- does, as its tool does: the entrance then ends at the street node
	-- beside it, the junction built above (refreshConstruction, build 40408:
	-- the same edge the tool proposed). So every game refreshes it at once,
	-- for free, as part of this action.
	local con = constructionAt({ file = build.file, at = build.transform.origin })
	return run(api.cmd.makeWorldBuildProposalCmd(refreshOf(con), nil, true, false))
end

-- ---------------------------------------------------------------- roads
--
-- A road or track build (tpf3mp_proto action::Polyline) as a SimpleProposal's
-- street proposal, as the game's own scripted track builder makes one
-- (mission/tasks/auto_builder/track_builder.tl): new nodes and edges with
-- negative ids, existing nodes by their own. A vertex resolves as the
-- originator's tool resolved it: New; the existing node of its network
-- within 1.5 m horizontally and 0.5 m in height, the nearest; or a split of the existing edge
-- between the nodes at its ends, cut in two at the vertex into halves that
-- keep the edge's own component, their tangents scaled to the part of the
-- curve each covers. The edges and nodes a build removes are found the same
-- way: an edge by the nodes at its ends, a node by its position. Each link
-- is the build's street or track, or the kind it names: a piece of the
-- street it joins, rebuilt through the new junction, keeps that street's.
--
-- Every game has the same world, so every game resolves alike; anything that
-- resolves to nothing fails the whole build, in every game.

local function module(name)
	local loaded = package and package.loaded and package.loaded["tpf3mp." .. name]
	if loaded then return loaded end
	if ug_require then return ug_require("tpf3mp_1::/scripts/tpf3mp/" .. name .. ".lua") end
	return require("tpf3mp." .. name)
end

local geom = module("geom")
local junctions = module("junctions")

-- How near an existing node a vertex resolving to it is, horizontally.
local NODE_TOLERANCE = 1.5
-- How near its node each end of a named edge is: the ends are the node's own
-- positions, rounded to the millimetre.
local END_TOLERANCE = 0.5
-- How far above or below its node a vertex or an end naming it is. Each is
-- the node's own position, rounded to the millimetre, so its height is the
-- node's; a track or road stacked over another stands metres apart from it,
-- and without the height a bulldoze of the upper one removed the lower.
local HEIGHT_TOLERANCE = 0.5

local function arr(v) return { v.x or v[1], v.y or v[2], v.z or v[3] } end
local function vec(p) return api.type.Vec3f.new(p[1], p[2], p[3]) end
local function scaled(p, k) return { p[1] * k, p[2] * k, p[3] * k } end

-- The game's enums, under api.type.enum ("enum" is a word in Teal, which
-- writes api.type["enum"]).
local function enum(name)
	local e = api.type.enum and api.type.enum[name]
	if e == nil then error("no api.type.enum." .. name) end
	return e
end

-- A resource's id by its name; the game answers -1 for none.
local function find(rep, name)
	local id = api.res[rep].find(name)
	if type(id) ~= "number" or id < 0 then error("no " .. rep .. " resource " .. tostring(name)) end
	return id
end

-- The nodes of a network: each node with an edge of it, and its position.
-- Held while it is read (on TPF2 a pairs() straight off the call let the
-- GC free the map mid-loop).
local function readNodes(network)
	local t0 = costs and clock()
	local streets = api.engine.system.streetSystem
	local map
	if network == "Track" then map = streets.getNode2TrackEdgeMap() else map = streets.getNode2StreetEdgeMap() end
	local nodes = {}
	for node in pairs(map) do
		local c = api.engine.getComponent(node, api.type.ComponentType.BASE_NODE)
		if c and c.position then nodes[#nodes + 1] = { id = node, pos = arr(c.position) } end
	end
	spend("nodes " .. tostring(network), t0, #nodes)
	return nodes
end

-- How far past a search's distance the octree is asked (nodesNear): the
-- float rounding of the edges' boxes.
local NEAR_MARGIN = 2

-- The existing edges whose boxes reach within `reach` of `p` horizontally,
-- at any height, as the engine's octree holds them: their ids, each once;
-- nil where this game has no octree to ask or it did not answer. Only ids
-- are collected in the engine's callback (tpf3mp/lanes.lua, spatial): a
-- Lua error there is fatal.
local function edgesNear(p, reach)
	local octree = api.engine.system and api.engine.system.octreeSystem
	if not (octree and octree.findIntersectingEntities and api.type.Box3) then return nil end
	local found = {}
	local ok, out = pcall(function()
		local box = api.type.Box3.new(api.type.Vec3f.new(p[1] - reach, p[2] - reach, -1000000),
			api.type.Vec3f.new(p[1] + reach, p[2] + reach, 1000000))
		octree.findIntersectingEntities(box, function(e) found[#found + 1] = e end)
		local edges, seen = {}, {}
		for _, e in ipairs(found) do
			if not seen[e] then
				seen[e] = true
				if api.engine.getComponent(e, api.type.ComponentType.BASE_EDGE) then edges[#edges + 1] = e end
			end
		end
		return edges
	end)
	if not ok then return nil end
	return out
end

-- The nodes of `network` within `tol` of `p` horizontally, as readNodes
-- gives them, found through the engine's octree instead of every node of
-- the network (2026-10-16: a late game's street network took 130-150 ms to
-- read whole in the GUI, once for each preview of another player's road,
-- several a second). Every node of the network has an edge of it, which
-- runs through the node's position, so the edges the octree finds there
-- (edgesNear) name such nodes at one of their ends; each is then read
-- afresh, as readNodes reads it, and kept where it is near enough. Nil
-- where the octree cannot say: then the caller reads them all. For
-- previews only (nodesOfNetwork).
local function nodesNear(network, p, tol)
	local t0 = costs and clock()
	local ids = edgesNear(p, tol + NEAR_MARGIN)
	if ids == nil then return nil end
	local ok, nodes = pcall(function()
		local C = api.type.ComponentType
		local streets = api.engine.system.streetSystem
		local seen, out = {}, {}
		for _, e in ipairs(ids) do
			local edge = api.engine.getComponent(e, C.BASE_EDGE)
			for _, node in ipairs(edge and { edge.node0, edge.node1 } or {}) do
				if not seen[node] then
					seen[node] = true
					local segments
					if network == "Track" then segments = streets.getNodeTrackSegments(node)
					else segments = streets.getNodeStreetSegments(node) end
					local c = segments and #segments > 0 and api.engine.getComponent(node, C.BASE_NODE)
					if c and c.position then
						local pos = arr(c.position)
						local dx, dy = pos[1] - p[1], pos[2] - p[2]
						if dx * dx + dy * dy <= tol * tol then out[#out + 1] = { id = node, pos = pos } end
					end
				end
			end
		end
		return out
	end)
	spend("near " .. tostring(network), t0, #ids)
	if not ok then return nil end
	return nodes
end

-- A network's nodes for one action of the kinds a preview shows, as
-- nearest and edgeBetween take them: a function of a place `p`, a distance
-- `tol` and `whole`, which answers a list of the network's nodes. An action the room
-- applies reads every node of the network (readNodes, once an action), as
-- it always did: the octree is not known to hold every edge, nor one built
-- earlier in the same update, and an edge it missed could make one game
-- choose another node, or pass a node it should refuse, than another game
-- (2026-10-16, Codex). A preview (dry), which nothing of reaches the world,
-- takes the nodes the octree finds near `p` (nodesNear) and reads them all
-- only where it finds none near enough (`whole`): a whole read cost
-- 130-150 ms of each preview of another player's road in the GUI.
local function nodesOfNetwork(network)
	local all
	return function(p, tol, whole)
		if dry and not whole then
			local near = nodesNear(network, p, tol)
			if near ~= nil then return near end
		end
		if all == nil then all = readNodes(network) end
		return all
	end
end

-- The node of `list` within `tol` horizontally and HEIGHT_TOLERANCE
-- vertically, nearest in three dimensions. Refuse a tie rather than choose
-- using this game's entity ids (D8).
local function nearestIn(list, p, tol)
	local best, bestD, tie
	for _, n in ipairs(list) do
		local dx, dy, dz = n.pos[1] - p[1], n.pos[2] - p[2], n.pos[3] - p[3]
		local horizontal = dx * dx + dy * dy
		if horizontal <= tol * tol and math.abs(dz) <= HEIGHT_TOLERANCE then
			local d = horizontal + dz * dz
			if bestD == nil or d < bestD then
				best, bestD, tie = n, d, false
			elseif d == bestD then
				tie = true
			end
		end
	end
	if tie then
		error(string.format("two nodes as near (%.3f, %.3f, %.3f)", p[1], p[2], p[3]), 0)
	end
	return best
end

-- `nodes` may be a list or a network's preview search. Applied builds pass
-- the complete list so a missing octree edge cannot change their outcome.
local function nearest(nodes, p, tol)
	if type(nodes) ~= "function" then return nearestIn(nodes, p, tol) end
	return nearestIn(nodes(p, tol), p, tol) or nearestIn(nodes(p, tol, true), p, tol)
end

-- The existing edge of `network` between the nodes at `a` and `b`: its id,
-- component and geometry (ends a and b, tangents ta and tb, as geom.lua takes
-- edges), oriented as the game has it. The lowest id if there are several.
local function edgeBetween(nodes, network, a, b)
	local na, nb = nearest(nodes, a, END_TOLERANCE), nearest(nodes, b, END_TOLERANCE)
	if na == nil or nb == nil or na.id == nb.id then return nil end
	local streets = api.engine.system.streetSystem
	local ids
	if network == "Track" then ids = streets.getNodeTrackSegments(na.id) else ids = streets.getNodeStreetSegments(na.id) end
	local found
	for i = 1, (ids and #ids or 0) do
		local id = ids[i]
		local c = api.engine.getComponent(id, api.type.ComponentType.BASE_EDGE)
		if c and ((c.node0 == na.id and c.node1 == nb.id) or (c.node0 == nb.id and c.node1 == na.id))
			and (found == nil or id < found.id) then
			local n0, n1 = na, nb
			if c.node0 == nb.id then n0, n1 = nb, na end
			found = { id = id, comp = c, node0 = n0.id, node1 = n1.id, a = n0.pos, b = n1.pos,
				ta = arr(c.tangent0), tb = arr(c.tangent1) }
		end
	end
	return found
end

local STRUCTURE = { Ground = "NORMAL", Bridge = "BRIDGE", Tunnel = "TUNNEL" }

-- A link's lanes: its template's, or the ones the tool made (a tram track, a
-- bus lane: a lane's transport modes on TF3). The game has no constructor
-- for a lane, so each is a copy of one of the template's (read afresh, so
-- no two are one), set to what the link says.
-- The transport modes a lane names (api.type.enum.TransportMode, 0 to 15).
local MODES = 16
local function lanesFor(link, t)
	if link.lanes == nil or #link.lanes == 0 then return t.laneConfigs end
	local out = {}
	for i, l in ipairs(link.lanes) do
		local fresh = t.laneConfigs
		local lane = fresh[math.min(i, #fresh)]
		if lane == nil then error("a lane its template has none to make it from", 0) end
		lane.speed, lane.width, lane.height, lane.offset = l.speed, l.width, l.height, l.offset
		lane.forward = l.forward == true
		-- Every mode, true or false. Build 40408 reads a lane's modes keyed
		-- from 0 (the TransportMode value) but takes them as a Lua array,
		-- from 1: mode m at m + 1. Keyed from 0 they land one mode off, and
		-- a sidewalk that carries vehicles failed every game's build, then
		-- crashed its simulation (TransportNetworkSystem, `person0 ==
		-- person1`; 2026-10-01).
		local modes = {}
		for m = 0, MODES - 1 do modes[m + 1] = math.floor(l.modes / 2 ^ m) % 2 == 1 end
		lane.transportModes = modes
		out[i] = lane
	end
	return out
end

-- Adds the polyline's nodes and edges, and its removals, to `proposal`'s
-- street proposal. `network`, `templateName` and `style` are the build's
-- own kind, for the links that name none; nil for a construction's
-- streets, whose every link names its kind. With `dangling` true (a
-- construction's streets), peel back complete branches ending at new
-- vertices: the construction makes its own entrance and internal track;
-- and every edge it removes or splits must be the acting company's or no
-- company's (D21), as a bulldozed one. `gone` names the edges an edit's old
-- construction takes with it (its frozen edges): the polyline may not
-- remove or split them, and no junction's settings may name them. Removing only the outermost links
-- leaves duplicate track inside a branched depot (Steam 40408). Existing
-- nodes and splits anchor the external network and are never peeled off.
function apply.ownStreets(polyline)
	local links, skipped = polyline.links, {}
	local degree, incident = {}, {}
	for i = 0, #polyline.vertices - 1 do degree[i], incident[i] = 0, {} end
	for k, link in ipairs(links) do
		for _, i in ipairs({ link.from, link.to }) do
			degree[i] = degree[i] + 1
			incident[i][#incident[i] + 1] = k
		end
	end
	local function loose(i) return polyline.vertices[i + 1].resolve == "New" and degree[i] == 1 end
	local queue, removed = {}, {}
	for i = 0, #polyline.vertices - 1 do if loose(i) then queue[#queue + 1] = i end end
	local head = 1
	while head <= #queue do
		local i = queue[head]
		head = head + 1
		for _, k in ipairs(incident[i]) do
			if not removed[k] then
				removed[k] = true
				local link = links[k]
				local other = link.from == i and link.to or link.from
				degree[i], degree[other] = degree[i] - 1, degree[other] - 1
				if loose(other) then queue[#queue + 1] = other end
			end
		end
	end
	links = {}
	for k, link in ipairs(polyline.links) do if not removed[k] then links[#links + 1] = link end end
	for i, v in ipairs(polyline.vertices) do skipped[i] = v.resolve == "New" and degree[i - 1] == 0 end
	return links, skipped
end

-- Leave the construction's own street and track junctions to its build.
-- Both capture and replay use this rule, so the originator does not send
-- settings for nodes and edges that no replica builds separately.
function apply.ownJunctions(polyline)
	local links, skipped = apply.ownStreets(polyline)
	local keep, ownEdges, ownNodes = {}, {}, {}
	for _, link in ipairs(links) do keep[link] = true end
	local function place(i)
		local p = arr(polyline.vertices[i + 1].pos)
		return { x = p[1], y = p[2], z = p[3] }
	end
	for _, link in ipairs(polyline.links) do
		if not keep[link] then
			local net = link.kind and link.kind.network
			ownEdges[#ownEdges + 1] = { network = net, ends = { a = place(link.from), b = place(link.to) } }
			for _, i in ipairs({ link.from, link.to }) do
				if skipped[i + 1] then ownNodes[#ownNodes + 1] = { network = net, at = place(i) } end
			end
		end
	end
	return junctions.without(polyline.junctions, ownNodes, ownEdges)
end

-- A construction whose own tracks the tool joined onto the ends of tracks
-- that exist (2026-10-10: an underpass mod's eight tracks onto eight track
-- ends beside a station, refused in every game though its tool built it,
-- "Construction Not Possible"). Built alone, as every game builds a
-- construction, its tracks end where those ends are, and only the tool
-- welds a construction's track ends onto existing nodes. So, when the game
-- refuses the construction alone (`refused`, its words) and the action's
-- connection is the construction's own track, joined only at existing
-- track nodes, every game takes the track pieces at those nodes away and
-- builds the construction in one proposal, then lays the pieces again at
-- once, each ending on the construction's own track end where its old end
-- was: joined, as the tool joined them. A piece keeps its form, kind,
-- lanes, decorations and owner, all read before anything changes (the
-- game's component of a removed edge is not to be read afterwards). A
-- piece with a stop or signal on it, a bridge or tunnel, another company's
-- piece, or a junction at its far end set by hand (its turns, its lights or
-- a double slip), and the build is refused as the game refused it; the
-- settings at a far end are the game's own otherwise, and go with the
-- pieces. Where the construction has no track end at such a place, the
-- piece comes back to a node of its own there, unjoined. The pieces are
-- laid again for free, as the game's refresh of a construction is: they
-- were the player's already; what still fails is logged with the places,
-- the same in every game, and the construction stands. Returns what the
-- build's command answered.
function rejoinConstruction(build, entity, context, refused)
	local polyline = build.connection
	local kept = apply.ownStreets(polyline)
	if #kept > 0 or #(polyline.removals or {}) > 0 or #(polyline.removed_nodes or {}) > 0 then
		error(refused, 0)
	end
	local C = api.type.ComponentType
	local streets = api.engine.system.streetSystem
	local engine = module("engine")
	local function count(ids) return ids and #ids or 0 end
	-- The existing track nodes the connection joins, in its order.
	local before = readNodes("Track")
	local anchors, anchorAt = {}, {}
	for i, v in ipairs(polyline.vertices) do
		local r = v.resolve
		if type(r) == "table" and (r.Split or (r.Node and r.Node ~= "Track")) then error(refused, 0) end
		if type(r) == "table" and r.Node then
			-- At its own level: a track's node right above or below (an
			-- overpass) is another place.
			local at = arr(v.pos)
			local level = {}
			for _, n in ipairs(before) do
				if math.abs(n.pos[3] - at[3]) <= 2 then level[#level + 1] = n end
			end
			local n = nearest(level, at, NODE_TOLERANCE)
			if n == nil then error("no Track node at vertex " .. i, 0) end
			if count(streets.getNodeStreetSegments(n.id)) > 0 then error(refused, 0) end
			if not anchorAt[n.id] then
				anchorAt[n.id] = n.pos
				anchors[#anchors + 1] = n.id
			end
		end
	end
	if #anchors == 0 then error(refused, 0) end
	-- The pieces at those nodes, by id, each read whole now.
	local pieces, taken = {}, {}
	for _, node in ipairs(anchors) do
		local ids = streets.getNodeTrackSegments(node)
		for i = 1, count(ids) do
			local id = ids[i]
			if not taken[id] then
				taken[id] = true
				pieces[#pieces + 1] = { id = id }
			end
		end
	end
	table.sort(pieces, function(a, b) return a.id < b.id end)
	for _, p in ipairs(pieces) do
		mine(p.id, "track")
		local c = api.engine.getComponent(p.id, C.BASE_EDGE)
		if c == nil then error("no edge component on " .. tostring(p.id), 0) end
		if #(c.objects or {}) > 0 then error(refused .. " (a stop or signal on a track it joins)", 0) end
		if c.type ~= enum("BaseEdgeType").NORMAL then error(refused .. " (a bridge or tunnel it joins)", 0) end
		p.owner = require_companies().ownerOf(api, p.id)
		p.n0, p.n1 = c.node0, c.node1
		p.t0, p.t1 = arr(c.tangent0), arr(c.tangent1)
		p.template, p.style = c.roadTemplate, c.roadStyle
		p.locked = c.roadDevelopmentLocked == true
		p.lanes = engine.lanesOf(c)
		p.decorations = engine.decorationsOf(c)
		for k, node in ipairs({ p.n0, p.n1 }) do
			local n = api.engine.getComponent(node, C.BASE_NODE)
			if n == nil then error("no node component on " .. tostring(node), 0) end
			p["at" .. (k - 1)] = arr(n.position)
		end
	end
	-- The nodes that go: those, and a far end left with no edge; the
	-- settings at a far end that stays go too, as they name the pieces,
	-- unless the player set them.
	local gone, goneList = {}, {}
	for _, node in ipairs(anchors) do
		gone[node] = true
		goneList[#goneList + 1] = node
	end
	local auto = enum("TrafficLightPreference").AUTO
	local orphan, configs, seen = {}, {}, {}
	for _, p in ipairs(pieces) do
		for _, node in ipairs({ p.n0, p.n1 }) do
			if not gone[node] and not seen[node] then
				seen[node] = true
				local ids, left = streets.getNodeTrackSegments(node), count(streets.getNodeStreetSegments(node))
				for i = 1, count(ids) do if not taken[ids[i]] then left = left + 1 end end
				if left == 0 then
					orphan[node] = true
					gone[node] = true
					goneList[#goneList + 1] = node
				else
					local c = api.engine.getComponent(node, C.BASE_NODE_CONFIG)
					if c ~= nil then
						local set = {}
						pcall(function()
							set = { c.userModifiedLaneConnections == true, c.userModifiedTrafficLightStates == true,
								c.doubleSlipSwitch == true, c.trafficLightPreference ~= auto }
						end)
						for _, by in ipairs(set) do
							if by then error(refused .. " (a junction set by hand where it joins)", 0) end
						end
						configs[#configs + 1] = node
					end
				end
			end
		end
	end
	table.sort(configs)

	-- The pieces away and the construction built, as one build.
	local first = api.type.SimpleProposal.new()
	first.constructionsToAdd = { entity }
	local removed = {}
	for i, p in ipairs(pieces) do removed[i] = p.id end
	first.streetProposal.edgesToRemove = removed
	first.streetProposal.nodesToRemove = goneList
	if #configs > 0 then first.streetProposal.nodeConfigsToRemove = configs end
	if refusal(first, context) then error(refused, 0) end
	log("joining " .. tostring(build.file) .. " onto track ends " .. table.concat(anchors, ",")
		.. ": its track pieces " .. table.concat(removed, ",") .. " laid again on it")
	local built = run(api.cmd.makeWorldBuildProposalCmd(first, context, true, true))

	-- The pieces again, as a track build lays them. An end that was one of
	-- those nodes ends on the construction's own new track node there, by
	-- its entity: the one track node within a few centimetres, height
	-- included, that did not exist before. A far end that stays is itself;
	-- one that went comes back as a node of its own. The construction
	-- stands, in every game alike, whatever comes of this.
	local vertices = {}
	local laid, why = pcall(function()
		local existed = {}
		for _, n in ipairs(before) do existed[n.id] = true end
		local after = readNodes("Track")
		local index, unjoined = {}, {}
		local function vertex(old, at)
			if index[old] then return index[old] end
			local resolve = "New"
			if anchorAt[old] then
				local found
				for _, n in ipairs(after) do
					if not existed[n.id] and math.abs(n.pos[1] - at[1]) <= 0.05 and math.abs(n.pos[2] - at[2]) <= 0.05
						and math.abs(n.pos[3] - at[3]) <= 0.5 then
						if found then error("two track ends of the new construction where track " .. old .. " ended", 0) end
						found = n.id
					end
				end
				if found then resolve = { Entity = found } else unjoined[#unjoined + 1] = tostring(old) end
			elseif not orphan[old] then
				resolve = { Entity = old }
			end
			vertices[#vertices + 1] = { pos = { x = at[1], y = at[2], z = at[3] }, resolve = resolve }
			index[old] = #vertices - 1
			return index[old]
		end
		local links = {}
		for _, p in ipairs(pieces) do
			local function t(a) return { x = a[1], y = a[2], z = a[3] } end
			links[#links + 1] = { from = vertex(p.n0, p.at0), to = vertex(p.n1, p.at1),
				tangent0 = t(p.t0), tangent1 = t(p.t1), structure = "Ground", decorations = p.decorations,
				locked = p.locked, owned = p.owner ~= nil, lanes = p.lanes,
				kind = { network = "Track", template = p.template, style = p.style } }
		end
		if #unjoined > 0 then
			log("the new " .. tostring(build.file) .. " has no track end where track " .. table.concat(unjoined, ",")
				.. " ended: laid again there unjoined")
		end
		local second = api.type.SimpleProposal.new()
		networkInto(second, "Track", nil, nil, { vertices = vertices, links = links, removals = {},
			removed_nodes = {}, junctions = {} })
		-- For free, as the game's refresh of a construction is: the pieces
		-- were the player's already, and the tool only joined onto them.
		local refused2 = refusal(second, nil)
		if refused2 then error(refused2, 0) end
		run(api.cmd.makeWorldBuildProposalCmd(second, nil, true, false))
	end)
	if not laid then
		local places = {}
		for i, v in ipairs(vertices) do
			places[i] = string.format("%s(%.1f,%.1f,%.1f)", type(v.resolve) == "table" and "node" or "new",
				v.pos.x, v.pos.y, v.pos.z)
		end
		log("the track pieces at the new " .. tostring(build.file) .. " could not be laid again: " .. tostring(why)
			.. " [" .. table.concat(places, " ") .. "]")
	end
	return built
end

-- How near where a kept signal stood, along its track, the signal of its
-- construction there is (as a PlaceSignals' removed one).
local KEPT_TOLERANCE = 0.25

-- An edge's length along its curve, in metres (its Hermite curve in 32
-- steps).
local function edgeLength(e)
	local length, last = 0, e.a
	for i = 1, 32 do
		local p = geom.hermitePos(e.a, e.ta, e.b, e.tb, i / 32)
		local dx, dy, dz = p[1] - last[1], p[2] - last[2], (p[3] or 0) - (last[3] or 0)
		length = length + math.sqrt(dx * dx + dy * dy + dz * dz)
		last = p
	end
	return length
end

-- The signals the polyline's links keep from the edges it removes
-- (action::KeptSignal), each found on its removal's edge as the world
-- stands, one for one, and each checked as the capture checked it: a track
-- signal, no waypoint (its signal list says, as tpf3mp/engine.lua's capture
-- reads it), the acting company's or no company's, on a link of track that
-- goes on from one of its old track's ends, the same way, or runs where it
-- stood, the same way (geom.keepsSignal, as the capture asks).
-- Anything missing or ambiguous refuses the whole build, in every game
-- alike. Returns { link, entity, kind, pos } for each.
local function keptSignals(polyline, removalEdges, network, at, dangling)
	local out, seen = {}, {}
	local any = false
	for _, link in ipairs(polyline.links) do
		if link.kept and #link.kept > 0 then any = true end
	end
	if not any then return out end
	if dangling then error("a construction's streets keep no signals", 0) end
	local C = api.type.ComponentType
	local SIGNAL = enum("EdgeObjectType").SIGNAL
	local lengths = {}
	for j, link in ipairs(polyline.links) do
		for n, kept in ipairs(link.kept or {}) do
			local what = "link " .. j .. "'s signal " .. n
			local r = polyline.removals[kept.removal + 1]
			local e = removalEdges[kept.removal + 1]
			if r == nil or e == nil then error(what .. ": no such removal", 0) end
			local own = link.kind and link.kind.network or network
			if r.network ~= "Track" or own ~= "Track" then error(what .. ": not on track", 0) end
			-- The place is from the removal's `a` end; this game's edge may
			-- run from its `b`.
			local ea = arr(r.ends.a)
			local dx, dy = e.a[1] - ea[1], e.a[2] - ea[2]
			local flipped = dx * dx + dy * dy > END_TOLERANCE * END_TOLERANCE
			local want = kept.at
			if flipped then want = 1 - want end
			lengths[e.id] = lengths[e.id] or edgeLength(e)
			local best, bestComp
			for _, o in ipairs(e.comp.objects or {}) do
				local c = o[2] == SIGNAL and not seen[o[1]] and api.engine.getComponent(o[1], C.EDGE_OBJECT)
				if c and c.edgeObjectConstruction == kept.model
					and math.abs(c.param - want) * lengths[e.id] <= KEPT_TOLERANCE then
					if best ~= nil then error(what .. ": two signals where one is kept", 0) end
					best, bestComp = o[1], c
				end
			end
			if best == nil then error(what .. ": no " .. tostring(kept.model) .. " where it is kept", 0) end
			local list = api.engine.getComponent(best, C.SIGNAL_LIST)
			local signals = list and list.signals or {}
			if #signals == 0 then error(what .. ": a signal with no signal list", 0) end
			for i = 1, #signals do
				local kind = signals[i].type
				if type(kind) ~= "number" or kind == api.type.Signal.Type.WAYPOINT then
					error(what .. ": a waypoint, which lines name, kept on another edge", 0)
				end
			end
			mine(e.id, "track")
			mine(best, "signal")
			-- Checked along the removal as the room names it, from its `a`
			-- end, so every game decides alike whichever way its own edge
			-- runs.
			local from = e
			if flipped then from = { a = e.b, b = e.a, ta = scaled(e.tb, -1), tb = scaled(e.ta, -1) } end
			local to = { a = at[link.from + 1], b = at[link.to + 1], ta = arr(link.tangent0), tb = arr(link.tangent1) }
			local ok, why = geom.keepsSignal(from, kept.at, to)
			if not ok then error(what .. ": " .. why, 0) end
			seen[best] = true
			local pos
			local t = bestComp.transf
			if t and type(t[13]) == "number" then pos = { t[13], t[14], t[15] } end
			out[#out + 1] = { link = j, entity = best, kind = SIGNAL, pos = pos }
		end
	end
	return out
end

-- After the build, each kept signal is still there, where it stood: the
-- game moves a signal onto the piece of track that takes it. One gone is a
-- world this mod cannot account for, so the room's replay stops
-- (replayFatal); one moved is said in the log.
local function settleKept(carried)
	local C = api.type.ComponentType
	local farthest
	for _, c in ipairs(carried) do
		local ok, comp = pcall(function() return api.engine.getComponent(c.entity, C.EDGE_OBJECT) end)
		if not ok or comp == nil then
			error({ replayFatal = "kept signal " .. tostring(c.entity) .. " is gone after the build" }, 0)
		end
		local t = comp.transf
		if c.pos and t and type(t[13]) == "number" then
			local dx, dy, dz = t[13] - c.pos[1], t[14] - c.pos[2], t[15] - c.pos[3]
			local moved = math.sqrt(dx * dx + dy * dy + dz * dz)
			if moved > 0.5 then
				log(string.format("kept signal %d moved %.2f m in the build", c.entity, moved))
			end
			if farthest == nil or moved > farthest then farthest = moved end
		end
	end
	if #carried > 0 then
		log("kept " .. #carried .. " signal(s) on the track's new pieces"
			.. (farthest and string.format(", the farthest moved %.2f m", farthest) or ", where they stand unread"))
	end
end

function networkInto(proposal, network, templateName, style, polyline, dangling, gone)
	local links, skipped = polyline.links, {}
	local settings = polyline.junctions
	if dangling then
		links, skipped = apply.ownStreets(polyline)
		local left
		settings, left = apply.ownJunctions(polyline)
		if #left > 0 then
			local ok, text = pcall(junctions.summary, { EditJunctions = { changes = left } })
			log("left to the construction: " .. (ok and text or (#left .. " junction(s)")))
		end
	end
	-- The junctions' settings go with it (junctions.into, below).
	polyline = { vertices = polyline.vertices, links = links, removals = polyline.removals,
		removed_nodes = polyline.removed_nodes, junctions = settings }
	local nodesOf = {}
	local function nodes(n)
		if nodesOf[n] == nil then nodesOf[n] = nodesOfNetwork(n) end
		return nodesOf[n]
	end
	local templates = {}
	local function template(name)
		if templates[name] == nil then
			templates[name] = api.res.streetTemplateRep.get(find("streetTemplateRep", name))
		end
		return templates[name]
	end
	local edgeType = enum("BaseEdgeType")

	-- Ids: the edges from -1, the links first and then two halves per split;
	-- the new nodes after them.
	local splits = 0
	for _, v in ipairs(polyline.vertices) do
		if type(v.resolve) == "table" and v.resolve.Split then splits = splits + 1 end
	end
	local nextEdge, nextNode = -1, -(#polyline.links + 2 * splits) - 1

	local nodesToAdd, edgesToAdd, edgesToRemove = {}, {}, {}
	-- Per vertex, the networks its edges are in and the existing nodes they
	-- join it to: a vertex on both a street and a track is where a level
	-- crossing may be laid, read back after the build (buildNetwork).
	local touch = {}
	local function touches(i, net, anchor)
		if net ~= "Street" and net ~= "Track" then return end
		local t = touch[i]
		if t == nil then
			t = { nets = {}, anchors = {} }
			touch[i] = t
		end
		t.nets[net] = true
		if type(anchor) == "number" and anchor >= 0 then t.anchors[#t.anchors + 1] = { node = anchor, network = net } end
	end
	-- The nodes at the ends of the edges removed, in order: their lane
	-- configurations name those edges, and go with them (below).
	local ends, endSeen = {}, {}
	local function removeEdge(e)
		edgesToRemove[#edgesToRemove + 1] = e.id
		for _, node in ipairs({ e.comp.node0, e.comp.node1 }) do
			if not endSeen[node] then
				endSeen[node] = true
				ends[#ends + 1] = node
			end
		end
	end
	local function addNode(p)
		local n = api.type.NodeAndEntity.new()
		n.entity = nextNode
		nextNode = nextNode - 1
		n.comp.position = vec(p)
		nodesToAdd[#nodesToAdd + 1] = n
		return n.entity
	end
	local function addEdge(kind, node0, node1, p0, p1, t0, t1, comp)
		local s = api.type.SegmentAndEntity.new()
		s.entity = nextEdge
		nextEdge = nextEdge - 1
		if comp ~= nil then s.comp = comp end
		s.type = kind
		s.comp.node0, s.comp.node1 = node0, node1
		s.comp.position0, s.comp.position1 = vec(p0), vec(p1)
		s.comp.tangent0, s.comp.tangent1 = vec(t0), vec(t1)
		edgesToAdd[#edgesToAdd + 1] = s
		return s
	end
	local function kindOf(n) if n == "Track" then return 1 end return 0 end

	-- The links' edges first, as their ids were counted.
	local links = {}
	local ids, at = {}, {}
	for i, v in ipairs(polyline.vertices) do at[i] = arr(v.pos) end
	for k, link in ipairs(polyline.links) do
		local own = link.kind and link.kind.network or network
		if own == nil then error("link " .. k .. " names no kind", 0) end
		-- Each end where the tool put it: its vertex, or beside it (a track
		-- laid onto a street's end starts on the street's tram lane, and the
		-- game joins the two there only; schema 27).
		local p0 = link.position0 and arr(link.position0) or at[link.from + 1]
		local p1 = link.position1 and arr(link.position1) or at[link.to + 1]
		links[k] = addEdge(kindOf(own), 0, 0, p0, p1, arr(link.tangent0), arr(link.tangent1))
	end

	for i, v in ipairs(polyline.vertices) do
		local p, r = at[i], v.resolve
		if skipped[i] then
			-- Left out, with its link.
		elseif r == "New" then
			ids[i] = addNode(p)
		elseif type(r) == "table" and r.Entity then
			-- A node by its entity: this game's own replay found it
			-- (rejoinConstruction); never in an action.
			ids[i] = r.Entity
		elseif type(r) == "table" and r.Node then
			local n = nearest(nodes(r.Node), p, NODE_TOLERANCE)
			if n == nil then error("no " .. r.Node .. " node at vertex " .. i) end
			ids[i] = n.id
			touches(i, r.Node)
		elseif type(r) == "table" and r.Split then
			local s = r.Split
			local e = edgeBetween(nodes(s.network), s.network, arr(s.ends.a), arr(s.ends.b))
			if e == nil then error("no " .. s.network .. " edge to split at vertex " .. i) end
			if gone and gone[e.id] then error("vertex " .. i .. " splits the old construction's own edge", 0) end
			if dangling then mine(e.id, "road or track") end
			if #(e.comp.objects or {}) > 0 then
				error("vertex " .. i .. " splits an edge with a stop or signal on it")
			end
			local tol = s.network == "Track" and geom.SPLIT_EPS_TRACK or geom.SPLIT_EPS
			local u, off = geom.parameterAt(e.a, e.ta, e.b, e.tb, p[1], p[2])
			local function from(q) local dx, dy = p[1] - q[1], p[2] - q[2] return math.sqrt(dx * dx + dy * dy) end
			if off > tol then error("vertex " .. i .. " is not on the edge it splits") end
			if from(e.a) < geom.SPLIT_MIN_DIST or from(e.b) < geom.SPLIT_MIN_DIST then
				error("vertex " .. i .. " splits the edge at its end")
			end
			local tm = geom.hermiteTangent(e.a, e.ta, e.b, e.tb, u)
			local mid = addNode(p)
			ids[i] = mid
			touches(i, s.network, e.node0)
			touches(i, s.network, e.node1)
			removeEdge(e)
			-- Each half the split edge's own component, read afresh, as the
			-- game's electrify task rebuilds an edge (electrify.tl).
			local component = api.type.ComponentType.BASE_EDGE
			addEdge(kindOf(s.network), e.node0, mid, e.a, p, scaled(e.ta, u), scaled(tm, u),
				api.engine.getComponent(e.id, component))
			addEdge(kindOf(s.network), mid, e.node1, p, e.b, scaled(tm, 1 - u), scaled(e.tb, 1 - u),
				api.engine.getComponent(e.id, component))
		else
			error("vertex " .. i .. " resolves as nothing this mod knows")
		end
	end

	for k, link in ipairs(polyline.links) do
		local s = links[k]
		s.comp.node0, s.comp.node1 = ids[link.from + 1], ids[link.to + 1]
		local net = link.kind and link.kind.network or network
		touches(link.from + 1, net, ids[link.to + 1])
		touches(link.to + 1, net, ids[link.from + 1])
		local structure, name = link.structure, "Ground"
		if type(structure) == "table" then name = next(structure) end
		s.comp.type = edgeType[STRUCTURE[name] or error("a link of structure " .. tostring(name))]
		if name == "Bridge" then
			s.comp.typeIndex = find("bridgeTypeRep", structure.Bridge)
		elseif name == "Tunnel" then
			s.comp.typeIndex = find("tunnelTypeRep", structure.Tunnel)
		else
			s.comp.typeIndex = -1
		end
		-- The build's own kind, or the kind the link names.
		local kind = link.kind or { network = network, template = templateName, style = style }
		local t = template(kind.template)
		s.comp.laneConfigs = lanesFor(link, t)
		s.comp.roadTemplate = kind.template
		s.comp.roadStyle = kind.style or t.streetStyle
		s.comp.roadType = kind.network == "Track" and enum("RoadType").TRACK or enum("RoadType").STREET
		-- A track's distance between its centre and its neighbours', its
		-- template's (StreetTemplate.trackDistance): without it the game lays
		-- no shared ballast bed or catenary with the tracks beside it, and
		-- the ground shows between them (2026-10-02, tracks laid side by
		-- side in a room). Every game reads the same template.
		if kind.network == "Track" then
			local ok, d = pcall(function() return t.trackDistance end)
			if ok and type(d) == "number" and d > 0 then s.comp.distance = d end
		end
		-- What the tool left on it: its decorations (by name, as every game
		-- numbers them), the towns' lock, and the acting company's ownership.
		local decorations = {}
		for _, d in ipairs(link.decorations or {}) do
			decorations[#decorations + 1] = { find("edgeDecorationRep", d.name), d.flag == true }
		end
		s.comp.edgeDecorations = decorations
		s.comp.roadDevelopmentLocked = link.locked == true
		-- A street's precedence at its ends, as the tool set it.
		if link.precedence ~= nil then
			if s.streetEdge == nil then
				local ok, street = pcall(function() return api.type.BaseEdgeStreet.new() end)
				s.streetEdge = ok and street or {}
			end
			s.streetEdge.precedenceNode0 = link.precedence.node0
			s.streetEdge.precedenceNode1 = link.precedence.node1
		end
		if link.owned == true then
			local ok = pcall(function() s.playerOwned.player = company() end)
			if not ok then
				local owned = api.type.PlayerOwned.new()
				owned.player = company()
				s.playerOwned = owned
			end
		end
	end

	-- An edge removed with its stops or signals leaves them pointing
	-- nowhere: on TPF2 that crashed every game at the same step
	-- (docs/BUILDING.md). So one with any is removed only where a link
	-- rebuilds it in place, between the same places in the same direction,
	-- which takes its objects under their own entities, or where links keep
	-- every one of its signals (`kept`: a track cut between its signals); as
	-- the capture demands, tpf3mp/engine.lua keptInPlace.
	local taken, removalEdges, removedIds = {}, {}, {}
	local function sameAt(a, b)
		return math.abs(a[1] - b[1]) < 0.05 and math.abs(a[2] - b[2]) < 0.05 and math.abs(a[3] - b[3]) < 0.05
	end
	for k, r in ipairs(polyline.removals or {}) do
		local e = edgeBetween(nodes(r.network), r.network, arr(r.ends.a), arr(r.ends.b))
		if e == nil then error("no " .. r.network .. " edge to remove (" .. k .. ")") end
		if gone and gone[e.id] then error("removal " .. k .. " is the old construction's own edge", 0) end
		if removedIds[e.id] then error("removal " .. k .. " names an edge another removal names", 0) end
		removedIds[e.id] = true
		if dangling then mine(e.id, "road or track") end
		removalEdges[k] = e
	end
	local objectsFor, claimed = {}, {}
	local function give(j, o)
		objectsFor[j] = objectsFor[j] or {}
		objectsFor[j][#objectsFor[j] + 1] = { o[1], o[2] }
	end
	local carried = keptSignals(polyline, removalEdges, network, at, dangling)
	for _, c in ipairs(carried) do
		claimed[c.entity] = true
		give(c.link, { c.entity, c.kind })
	end
	for k, e in ipairs(removalEdges) do
		local objects = e.comp.objects or {}
		local kept = 0
		for _, o in ipairs(objects) do if claimed[o[1]] then kept = kept + 1 end end
		if kept > 0 and kept < #objects then
			error("removal " .. k .. " has a signal a link keeps and an object no link takes", 0)
		elseif kept == 0 and #objects > 0 then
			local into
			for j, link in ipairs(polyline.links) do
				if not taken[j] and sameAt(at[link.from + 1], e.a) and sameAt(at[link.to + 1], e.b) then
					into = j
					break
				end
			end
			if into == nil then error("removal " .. k .. " has a stop or signal on it and no link rebuilds it") end
			taken[into] = true
			for _, o in ipairs(objects) do give(into, o) end
		end
		removeEdge(e)
	end
	for j, objects in pairs(objectsFor) do links[j].comp.objects = objects end

	local nodesToRemove, removedNode = {}, {}
	for k, n in ipairs(polyline.removed_nodes or {}) do
		local found = nearest(nodes(n.network), arr(n.at), END_TOLERANCE)
		if found == nil then error("no " .. n.network .. " node to remove (" .. k .. ")") end
		nodesToRemove[#nodesToRemove + 1] = found.id
		removedNode[found.id] = true
	end

	-- A node's lane configuration (BASE_NODE_CONFIG) names the edges at it,
	-- and the game cannot read a proposal that removes an edge a
	-- configuration still names (build 40408: "Unknown exception" from
	-- makeProposalData). So the configurations at the ends of the removed
	-- edges go too. junctions.into below adds their settings back with the
	-- replacement edges; a removed node takes its own configuration with it
	-- and may not be named for both.
	local configsToRemove = {}
	for _, node in ipairs(ends) do
		if not removedNode[node]
			and api.engine.getComponent(node, api.type.ComponentType.BASE_NODE_CONFIG) ~= nil then
			configsToRemove[#configsToRemove + 1] = node
		end
	end

	proposal.streetProposal.nodesToAdd = nodesToAdd
	proposal.streetProposal.edgesToAdd = edgesToAdd
	proposal.streetProposal.edgesToRemove = edgesToRemove
	if #nodesToRemove > 0 then proposal.streetProposal.nodesToRemove = nodesToRemove end
	if #configsToRemove > 0 then proposal.streetProposal.nodeConfigsToRemove = configsToRemove end
	-- A preview (dry) leaves the junctions' lane and light settings out:
	-- they draw nothing, and a snapped build's may name a node only its
	-- originator's tool has.
	if not dry then
		local t0 = costs and clock()
		local left = junctions.into(proposal, polyline.junctions, ends, mine, gone)
		spend("junctions", t0)
		if left and #left > 0 then
			log("left to the construction: the settings of " .. #left .. " junction(s) at its old edges")
		end
	end

	-- What is sent, in the log before it goes: an exception from the game
	-- does not always come back through pcall.
	local shape = {}
	for _, n in ipairs(nodesToAdd) do
		local p = n.comp.position
		shape[#shape + 1] = string.format("+n%d(%.1f,%.1f,%.1f)", n.entity, p.x, p.y, p.z)
	end
	for _, s in ipairs(edgesToAdd) do
		shape[#shape + 1] = "+e" .. s.entity .. "/" .. tostring(s.type) .. ":" .. tostring(s.comp.node0) .. ">"
			.. tostring(s.comp.node1) .. " " .. tostring(s.comp.roadTemplate)
	end
	shape[#shape + 1] = "-e" .. table.concat(edgesToRemove, ",") .. " -n" .. table.concat(nodesToRemove, ",")
		.. " -c" .. table.concat(configsToRemove, ",")
	-- The junction settings put in, each node's lanes and how many a tram's.
	for _, n in ipairs(proposal.streetProposal.nodeConfigsToAdd or {}) do
		local turns, trams = 0, 0
		pcall(function()
			local lanes = n.comp.laneConnections
			for i = 1, #lanes do
				turns = turns + 1
				if lanes[i].withTram then trams = trams + 1 end
			end
		end)
		shape[#shape + 1] = "+j" .. tostring(n.entity) .. "{lanes " .. turns .. " tram " .. trams .. "}"
	end
	log("building " .. table.concat(shape, " "))
	-- Second, the vertices' node ids, in this proposal's numbering, and the
	-- vertices on both networks: for the level crossings it lays
	-- (buildNetwork).
	local crossings = {}
	for i = 1, #polyline.vertices do
		local t = touch[i]
		if t and t.nets.Street and t.nets.Track and ids[i] ~= nil then
			crossings[#crossings + 1] = { vertex = i, at = at[i], id = ids[i], anchors = t.anchors }
		end
	end
	return carried, { ids = ids, vertices = #polyline.vertices, crossings = crossings }
end

-- The level crossings at a vertex on both networks (`c`, as networkInto
-- noted it), each once: on its own node when it is an existing one, and on
-- the nodes the existing nodes next to it reach within CROSSING_REACH of
-- it, each through an edge of its network. The game lays a crossing as
-- half, connector, half (docs/BUILDING.md, "Level crossings"), so a new
-- crossing's node may sit a few metres off the vertex, and only an edge
-- laid to the vertex reaches one there.
local CROSSING_REACH, CROSSING_RISE = 10, 3
local function crossingsAt(c)
	local streets = api.engine.system.streetSystem
	local system = api.engine.system.railRoadCrossingSystem
	local near, found, seen = {}, {}, {}
	if c.id >= 0 then near[1] = c.id end
	for _, a in ipairs(c.anchors) do
		local segments = (a.network == "Track" and streets.getNodeTrackSegments or streets.getNodeStreetSegments)(a.node)
		for j = 1, (segments and #segments or 0) do
			local e = api.engine.getComponent(segments[j], api.type.ComponentType.BASE_EDGE)
			local other = e and ((e.node0 == a.node and e.node1) or (e.node1 == a.node and e.node0))
			local b = other and api.engine.getComponent(other, api.type.ComponentType.BASE_NODE)
			local p = b and arr(b.position)
			if p then
				local dx, dy = p[1] - c.at[1], p[2] - c.at[2]
				if dx * dx + dy * dy <= CROSSING_REACH * CROSSING_REACH and math.abs(p[3] - c.at[3]) <= CROSSING_RISE then
					near[#near + 1] = other
				end
			end
		end
	end
	for _, node in ipairs(near) do
		local crossing = system.getRailroadCrossingForNode(node)
		if type(crossing) == "number" and crossing >= 0 and not seen[crossing] then
			seen[crossing] = true
			found[#found + 1] = crossing
		end
	end
	return found
end

local function buildNetwork(network, templateName, style, polyline, crossingType)
	-- A type this game does not offer is refused before anything is laid.
	local crossingIndex = crossingType ~= nil and module("crossings").typeIndex(crossingType) or nil
	local proposal = api.type.SimpleProposal.new()
	local carried, laid = networkInto(proposal, network, templateName, style, polyline)
	-- The level crossing type the player's toolbar picked (docs/HOOKS.md,
	-- "Level crossing types"), as the tool maps it: each node of the build's
	-- own polyline to -2 - type, the type "preferred", so a crossing that
	-- is there already keeps its own, and every crossing laid anew takes it.
	-- The tool maps only the polyline's crossing nodes; a vertex that joins
	-- no crossing is never looked up.
	if crossingIndex ~= nil then
		local map = {}
		for i = 1, laid.vertices do
			if laid.ids[i] ~= nil then map[laid.ids[i]] = -2 - crossingIndex end
		end
		proposal.streetProposal.node2rcType = map
		log("level crossings laid as " .. crossingType)
	end
	-- Paid by the player, as the tool builds; the town buildings in the way
	-- cleared, as the tool clears them (the capture lets only those through).
	local context = api.type.Context.new()
	context.player = company()
	context.gatherBuildings = true
	-- The crossings at the build's vertices on both networks before it, by
	-- their nodes: one the build extends keeps its type (preferred).
	local before, readBefore
	if crossingIndex ~= nil and not dry and #laid.crossings > 0 then
		local crossings = module("crossings")
		readBefore, before = pcall(function()
			local types = {}
			for _, c in ipairs(laid.crossings) do
				for _, crossing in ipairs(crossingsAt(c)) do
					local comp = crossings.component(crossing)
					for _, node in ipairs(crossings.nodes(comp)) do types[node] = comp.typeIndex end
				end
			end
			return types
		end)
		if not readBefore then log("the level crossings there do not read: " .. tostring(before)) end
	end
	local built = buildProposal(proposal, context)
	settleKept(carried)
	-- Each crossing at those vertices, read back: one laid anew of another
	-- type than the toolbar's, or one extended that lost its own, is said.
	-- Only said, since the build went and nothing here may fail it; the
	-- room's world checks read every crossing's type (tpf3mp/lanes.lua), so
	-- games that laid different ones are found there.
	if built == true and readBefore then
		local crossings = module("crossings")
		local read, why = pcall(function()
			for _, c in ipairs(laid.crossings) do
				local found = crossingsAt(c)
				if #found == 0 then log("no level crossing at vertex " .. c.vertex) end
				for _, crossing in ipairs(found) do
					local comp = crossings.component(crossing)
					local expected = crossingIndex
					for _, node in ipairs(crossings.nodes(comp)) do
						if before[node] ~= nil then expected = before[node] break end
					end
					if comp == nil or comp.typeIndex ~= expected then
						log("level crossing " .. crossing .. " at vertex " .. c.vertex .. " took type "
							.. tostring(comp and comp.typeIndex) .. ", not " .. tostring(expected)
							.. " (the toolbar's " .. crossingType .. " is " .. crossingIndex .. ")")
					end
				end
			end
		end)
		if not read then log("the level crossings laid do not read back: " .. tostring(why)) end
	end
	return built
end

function HANDLERS.BuildRoad(road)
	return buildNetwork("Street", road.street, road.style, road.polyline, road.crossing_type)
end

function HANDLERS.EditJunctions(edit)
	local proposal = api.type.SimpleProposal.new()
	junctions.into(proposal, edit.changes, {}, mine, nil, junctions.requireNarrow)
	local context = api.type.Context.new()
	context.player = company()
	return buildProposal(proposal, context)
end

-- The town buildings the game's removal of streets takes with them
-- (makeSegmentsRemoveProposal gathers them as the bulldozer did, through
-- the same street_util::FinishProposal: build 40408, read statically),
-- checked against those the action names, `named`: every one it removes
-- must be a town building named there, of its file within 2 m, and every
-- one named must be removed. So no game removes a building the player did
-- not see go, and none keeps one the player's did not (PLAN.md: no
-- demolition beyond what was asked). Raises otherwise; returns the
-- entities it removes, for the log. A removal whose list does not read
-- passes only when it names none (as before schema 15).
-- The asset groups near (x, y) that hold an asset of `model` there: the
-- game's octree first, every asset group where it gives none.
local function assetGroupsAt(model, x, y, z)
	local engine = module("engine")
	local GROUP = api.type.ComponentType.ASSET_GROUP
	local candidates = {}
	local ok, near = pcall(function()
		return api.engine.util.octree.findEntitiesInCircle(api.type.Vec2f.new(x, y), 1, GROUP)
	end)
	if ok and type(near) == "table" then candidates = near end
	-- TF3 (build 40408) refuses to list asset groups ("Cannot loop over this
	-- component type"): then the octree's answer, none, stands.
	if #candidates == 0 then
		local okAll, all = pcall(api.engine.getEntitiesWithComponent, GROUP)
		if okAll and type(all) == "table" then candidates = all end
	end
	local out = {}
	for i = 1, #candidates do
		local g = candidates[i]
		local okA, assets = pcall(engine.assetsOf, g)
		if okA then
			for _, a in ipairs(assets) do
				if engine.assetAt(a, model, x, y, z) then
					out[#out + 1] = { entity = g, assets = assets }
					break
				end
			end
		end
	end
	return out
end

-- Trees and other assets the asset bulldozer took out of their group
-- (action::Bulldoze::Assets): the one group here that holds exactly
-- `count` assets, the first and every one removed among them, removed and,
-- unless every asset of it went, built again from this game's own copy
-- without them, as the tool builds it (tpf3mp/engine.lua, captureAssets;
-- construction_builder_util::CreateProposalAddAsset, build 40408): one
-- construction entity at the world's origin whose desc is autoRemovable and
-- whose one subconstruction lists the assets kept, the thin ones then the
-- full ones, each its model's file and its world matrix. TF3's
-- Proposal.ConstructionEntity has no writable fileName (it reads the
-- desc's, empty as new() makes it), and its construction is a
-- Proposal.ConstructionResult, whose subconstructions are set. Any other
-- group here, or a removed asset this game cannot tell from another,
-- refuses it, so no game removes other trees than the player's. Paid by the
-- player's company, as the player's own build. The log says the group and
-- its assets before and after, in every game.
local function removeAssets(a, context)
	local engine = module("engine")
	local f = a.first
	local found = {}
	for _, g in ipairs(assetGroupsAt(f.model, f.at.x, f.at.y, f.at.z)) do
		if #g.assets == a.count then
			local used, all = {}, true
			for _, r in ipairs(a.removed) do
				if not engine.takeAsset(g.assets, used, r.model, r.at.x, r.at.y, r.at.z) then all = false break end
			end
			if all then found[#found + 1] = { entity = g.entity, assets = g.assets, used = used } end
		end
	end
	if #found ~= 1 then
		error(#found == 0 and ("no asset group of " .. a.count .. " assets with those trees here")
			or ("more than one asset group of " .. a.count .. " assets with those trees here"), 0)
	end
	local group = found[1]
	for _, r in ipairs(a.removed) do
		if engine.assetsAt(group.assets, r.model, r.at.x, r.at.y, r.at.z) > 1 then
			error(string.format("two assets of %s at %.3f, %.3f, %.3f here: which one went is not clear",
				tostring(r.model), r.at.x, r.at.y, r.at.z), 0)
		end
	end
	local P = api.type.Proposal
	local column = api.type.Vec4f.new
	local function mat4(m)
		return api.type.Mat4f.new(column(m[1], m[2], m[3], m[4]), column(m[5], m[6], m[7], m[8]),
			column(m[9], m[10], m[11], m[12]), column(m[13], m[14], m[15], m[16]))
	end
	-- The assets kept, in the group's order (thin, then full), as the tool
	-- lists them.
	local models = {}
	for i, asset in ipairs(group.assets) do
		if not group.used[i] then
			local tm = P.TransformedModel.new()
			tm.id = asset.model
			tm.transf = mat4(engine.assetMatrix(asset, a.mirrored))
			models[#models + 1] = tm
		end
	end
	local proposal = P.new()
	proposal.toRemove = { group.entity }
	if #models > 0 then
		local sub = P.Subconstruction.new()
		sub.models = models
		local ce = P.ConstructionEntity.new()
		local desc = ce.desc
		if desc == nil then error("this game makes no construction desc for an asset group", 0) end
		desc.autoRemovable = true
		ce.desc = desc
		local con = ce.construction
		if con == nil then error("this game makes no construction for an asset group", 0) end
		con.subconstructions = { sub }
		ce.construction = con
		ce.transf = mat4({ 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1 })
		ce.playerEntity = a.owned and company() or -1
		proposal.toAdd = { ce }
	end
	log(string.format("trees: asset group %d of %d assets, %d removed (%s at %.2f,%.2f), rebuilt with %d",
		group.entity, #group.assets, #a.removed, tostring(a.removed[1].model), a.removed[1].at.x,
		a.removed[1].at.y, #models))
	run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
	-- What stands now: the group that holds the first asset kept, and how
	-- many it holds; with none kept, whether a group still holds the first
	-- one removed (the same in every game, or the replay differed).
	local function holding(model, x, y, z, what)
		local now = assetGroupsAt(model, x, y, z)
		local counts = {}
		for _, g in ipairs(now) do counts[#counts + 1] = tostring(#g.assets) end
		return #now == 0 and ("no group holds the " .. what)
			or (#now .. " group(s) hold the " .. what .. ", of " .. table.concat(counts, ",") .. " assets")
	end
	local after
	for i, asset in ipairs(group.assets) do
		if not group.used[i] then
			after = holding(asset.model, asset.x, asset.y, asset.z, "first tree kept")
			break
		end
	end
	if after == nil then
		local r = a.removed[1]
		after = holding(r.model, r.at.x, r.at.y, r.at.z, "first tree removed")
	end
	log("trees: after the rebuild " .. after)
	return true
end

-- An asset brush stroke (action::AssetStroke): every group it replaces found
-- here as the asset bulldozer finds one, the one group that holds exactly
-- that many assets with its first one where it stood; then one build that
-- removes them and adds the stroke's groups, each as the tool builds one
-- (construction_builder_util::CreateProposalAddAsset, as removeAssets
-- rebuilds a group): a construction entity at the world's origin whose desc
-- is autoRemovable and whose one subconstruction lists the group's assets,
-- each its model's file and its world matrix. Owned by the player's company
-- and paid by it, as the player's own build. A replaced group missing or
-- ambiguous here refuses the stroke, in every game.
function HANDLERS.PlantAssets(t)
	local P = api.type.Proposal
	local removed = {}
	for i, r in ipairs(t.removed or {}) do
		local found = {}
		for _, g in ipairs(assetGroupsAt(r.first.model, r.first.at.x, r.first.at.y, r.first.at.z)) do
			if #g.assets == r.count then found[#found + 1] = g.entity end
		end
		if #found ~= 1 then
			error(string.format("%s asset group of %d assets with %s at %.2f, %.2f here (group %d the stroke replaces)",
				#found == 0 and "no" or "more than one", r.count, tostring(r.first.model), r.first.at.x, r.first.at.y, i), 0)
		end
		removed[#removed + 1] = found[1]
	end
	local identity = matrix({ basis = { 1, 0, 0, 0, 1, 0, 0, 0, 1 }, origin = { x = 0, y = 0, z = 0 } })
	local added, count = {}, 0
	for g, group in ipairs(t.groups or {}) do
		local models = {}
		for _, a in ipairs(group) do
			local file = t.models[a.model + 1]
			if file == nil then error("group " .. g .. " names a model the stroke does not", 0) end
			local tm = P.TransformedModel.new()
			tm.id = file
			tm.transf = matrix(a.transform)
			models[#models + 1] = tm
		end
		local sub = P.Subconstruction.new()
		sub.models = models
		local ce = P.ConstructionEntity.new()
		local desc = ce.desc
		if desc == nil then error("this game makes no construction desc for an asset group", 0) end
		desc.autoRemovable = true
		ce.desc = desc
		local con = ce.construction
		if con == nil then error("this game makes no construction for an asset group", 0) end
		con.subconstructions = { sub }
		ce.construction = con
		ce.transf = identity
		ce.playerEntity = company()
		added[#added + 1] = ce
		count = count + #models
	end
	local proposal = P.new()
	proposal.toRemove = removed
	proposal.toAdd = added
	local context = api.type.Context.new()
	context.player = company()
	run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
	log(string.format("asset brush applied: %d group(s) of %d asset(s), replacing %d group(s)", #added, count,
		#removed))
	return true
end

local function townBuildingsRemoved(proposal, named)
	local ok, removed = pcall(function()
		local l, out = proposal.toRemove, {}
		for i = 1, #l do out[i] = l[i] end
		return out
	end)
	if not ok or type(removed) ~= "table" then
		if #named > 0 then error("the game's removal does not say which town buildings it takes", 0) end
		return ""
	end
	local CONSTRUCTION = api.type.ComponentType.CONSTRUCTION
	local taken, ids = {}, {}
	for _, e in ipairs(removed) do
		local c = api.engine.getComponent(e, CONSTRUCTION)
		local file = c and tostring(c.fileName) or ("entity " .. tostring(e))
		if c == nil or #(c.townBuildings or {}) == 0 then
			error("the game would remove " .. file .. " with the streets", 0)
		end
		local t, best, bestD = c.transf, nil, nil
		for i, ref in ipairs(named) do
			if not taken[i] and ref.file == c.fileName then
				local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
				local d = dx * dx + dy * dy + dz * dz
				if d <= 4 and (bestD == nil or d < bestD) then best, bestD = i, d end
			end
		end
		if best == nil then error("the game would also remove the town building " .. file .. ", which the player's did not", 0) end
		taken[best] = true
		ids[#ids + 1] = tostring(e)
	end
	for i, ref in ipairs(named) do
		if not taken[i] then error("no town building " .. tostring(ref.file) .. " there to remove", 0) end
	end
	return table.concat(ids, ",")
end

-- The signals on the tracks a removal takes away go with them, in its
-- edgeObjectsToRemove: an edge removed with objects still on it leaves them
-- pointing nowhere (on TPF2 that crashed every game; docs/BUILDING.md).
-- makeSegmentsRemoveProposal answers a whole Proposal, its street part in
-- `proposal` (as the bulldozer's own). Its list may name them already; one
-- that names anything else refuses the removal, and one that leaves any
-- out is given them, and must then read back exactly those, else the
-- removal is refused too.
local function withSignalsRemoved(proposal, signals)
	local want, unique = {}, {}
	for _, id in ipairs(signals) do
		if not want[id] then unique[#unique + 1] = id end
		want[id] = true
	end
	signals = unique
	table.sort(signals)
	local function listed()
		local ok, out = pcall(function()
			local l, out = proposal.proposal.edgeObjectsToRemove, {}
			for i = 1, (l and #l or 0) do out[i] = l[i] end
			return out
		end)
		if not ok or type(out) ~= "table" then
			error("the game's removal does not say which signals it takes", 0)
		end
		return out
	end
	-- Exactly the signals, each once; or false, and raises for anything else.
	local function exactly(l)
		local seen = {}
		for _, id in ipairs(l) do
			if not want[id] then
				error("the game would remove an edge object " .. tostring(id) .. " the bulldozer did not", 0)
			end
			if seen[id] then error("the game's removal names signal " .. tostring(id) .. " twice", 0) end
			seen[id] = true
		end
		return #l == #signals
	end
	if not exactly(listed()) then
		pcall(function() proposal.proposal.edgeObjectsToRemove = signals end)
		if not exactly(listed()) then error("the game's removal will not take the signals on the tracks", 0) end
	end
	log("removing signals " .. table.concat(signals, ",") .. " with their tracks")
end

-- The bulldozer's removals, as the game makes them itself: a construction
-- with what is its own (createProposalRemove: its entrance edge and node, as
-- the bulldozer proposed them on build 40408; a town building alone), or
-- edges with the nodes they leave on their own and the town buildings along
-- them (makeSegmentsRemoveProposal; townBuildingsRemoved). Paid by the
-- player's company, as the tool removes (the context's player), town
-- buildings' demolition included; the town's reputation follows from the
-- same proposal, as the player's own build (playerInitiated: the game's
-- towns script, onPreBuildProposal), in every game at the same step.
function HANDLERS.Bulldoze(b)
	local context = api.type.Context.new()
	context.player = company()
	local proposals = api.engine.util.proposal
	local proposal
	if b.Construction then
		local con, construction = constructionAt(b.Construction)
		pcall(diagnosticConstruction, b.Construction, con, construction, acting and acting.roster)
		mine(con, "construction")
		proposal = proposals.createProposalRemove(con, context)
		if proposal == nil then error("the game will not remove the " .. tostring(b.Construction.file), 0) end
		log("removing " .. tostring(con) .. " " .. tostring(b.Construction.file))
	elseif b.Edges then
		local network = b.Edges.network
		local nodes = readNodes(network)
		local ids, signals = {}, {}
		for k, ends in ipairs(b.Edges.edges) do
			local e = edgeBetween(nodes, network, arr(ends.a), arr(ends.b))
			if e == nil then error("no " .. network .. " edge to remove (" .. k .. ")", 0) end
			-- A track's signals go with it, as the bulldozer removes them; a
			-- stop, or any object on a street, refuses it.
			for _, o in ipairs(e.comp.objects or {}) do
				if network ~= "Track" or o[2] ~= enum("EdgeObjectType").SIGNAL then
					error("edge " .. k .. " has a stop on it", 0)
				end
				mine(o[1], "signal")
				signals[#signals + 1] = o[1]
			end
			mine(e.id, "road or track")
			ids[#ids + 1] = e.id
		end
		proposal = proposals.makeSegmentsRemoveProposal(ids)
		if #signals > 0 then withSignalsRemoved(proposal, signals) end
		local gone = townBuildingsRemoved(proposal, b.Edges.buildings or {})
		log("removing " .. network .. " edges " .. table.concat(ids, ",")
			.. (gone ~= "" and (" and town buildings " .. gone) or ""))
	elseif b.EdgeObject then
		-- A simple proposal: the game's verdict first (buildProposal).
		return removeEdgeObject(b.EdgeObject.edge, { b.EdgeObject }, context, false)
	elseif b.EdgeObjects then
		return removeEdgeObject(b.EdgeObjects.edge, b.EdgeObjects.objects, context, true)
	elseif b.Assets then
		return removeAssets(b.Assets, context)
	else
		return false, "a bulldoze of no kind"
	end
	-- The game's verdict takes simple proposals only: a removal it refuses
	-- fails in the command's own answer (run).
	return run(api.cmd.makeWorldBuildProposalCmd(proposal, context, true, true))
end

function HANDLERS.BuildTrack(track)
	return buildNetwork("Track", track.track, track.style, track.polyline, track.crossing_type)
end

-- An upgrade tool's build, said in the log once every game built it
-- (tpf3mp/roads.lua upgradeSummary): the same line in every game.
for _, name in ipairs({ "BuildRoad", "BuildTrack" }) do
	local build = HANDLERS[name]
	HANDLERS[name] = function(body)
		local ok, why = build(body)
		if ok == true then
			local summarised, text = pcall(module("roads").upgradeSummary, { [name] = body })
			if summarised and text then log("upgrade applied: " .. text) end
		end
		return ok, why
	end
end


-- ------------------------------------------------------------ terraform
--
-- A terrain tool's stroke, as its height grid (tpf3mp/capture.lua,
-- capture.terraform). A script cannot fill a proposal's height grid (Lua's
-- GridVec2f has no setter, build 40408), so the hook does: the grid goes to
-- the hook (apply.terrain, which the game script sets to the link's
-- tpf3mp_native.terrain), then an empty proposal, the carrier, is sent as
-- the player's build, paid by the player as the tool's is; the hook fills
-- the carrier's height grid at its apply (crates/tpf3mp-hook/src/terrain.rs).
-- Then the hook is disarmed, and the action fails unless the carrier was
-- filled. No verdict first: the game's verdict reads the proposal as sent,
-- empty.
function HANDLERS.Terraform(t)
	if type(apply.terrain) ~= "function" then error("this hook cannot apply a terraform", 0) end
	local ok, resolution = pcall(function() return api.engine.terrain.getBaseResolution() end)
	local cell = ok and resolution and (resolution.x or resolution[1])
	if type(cell) ~= "number" or math.abs(cell - t.cell) > 1e-6 then
		error("a grid of " .. tostring(t.cell) .. " m cells; this map's are " .. tostring(cell), 0)
	end
	local x0, y0 = t.origin.x / t.cell, t.origin.y / t.cell
	if x0 ~= math.floor(x0) or y0 ~= math.floor(y0) then error("a grid that starts between cells", 0) end
	local width = t.columns
	local height = #t.cells / width
	local values, low, high = {}, nil, nil
	for i, c in ipairs(t.cells) do
		values[2 * i - 1], values[2 * i] = c.target, c.before
		low, high = math.min(low or c.target, c.target), math.max(high or c.target, c.target)
	end
	local armed, why = apply.terrain({ x0 = x0, y0 = y0, width = width, height = height, cells = values })
	if armed ~= true then error("the hook would not take the grid: " .. tostring(why), 0) end
	local context = api.type.Context.new()
	context.player = company()
	local sent, err = pcall(function()
		return run(api.cmd.makeWorldBuildProposalCmd(api.type.Proposal.new(), context, true, true))
	end)
	local filled = apply.terrain(nil)
	if not sent then error(err, 0) end
	if filled ~= true then error("the hook filled no build with the grid", 0) end
	log(string.format("terraform applied: %d by %d cells from cell (%d, %d), heights %.2f to %.2f m",
		width, height, x0, y0, low or 0, high or 0))
	return true
end

-- A terrain painter's stroke, as the grids the hook read (action::TerrainPaint,
-- the hook made it, crates/tpf3mp-hook/src/terrain.rs): as a terraform, the
-- stroke goes to the hook (apply.terrain) and an empty proposal, the carrier,
-- is sent as the player's build; the hook fills the carrier's materials and
-- mask grids at its apply. The action fails unless the carrier was filled.
function HANDLERS.Paint(t)
	if type(apply.terrain) ~= "function" then error("this hook cannot apply paint", 0) end
	local armed, why = apply.terrain(t)
	if armed ~= true then error("the hook would not take the paint: " .. tostring(why), 0) end
	local context = api.type.Context.new()
	context.player = company()
	local sent, err = pcall(function()
		return run(api.cmd.makeWorldBuildProposalCmd(api.type.Proposal.new(), context, true, true))
	end)
	local filled = apply.terrain(nil)
	if not sent then error(err, 0) end
	if filled ~= true then error("the hook filled no build with the paint", 0) end
	log("paint applied: " .. module("capture").paintSummary(t))
	return true
end

-- ---------------------------------------------------------------- stops
--
-- A stop is placed, or removed, as the stop tool and the bulldozer propose
-- it (tpf3mp/engine.lua): the edge removed and added again between the same
-- nodes, its own component read afresh (as the game's electrify task
-- rebuilds an edge, electrify.tl), so every stop and signal it had stays
-- under its own entity, re-parented with its station group and lines. A new
-- stop is `edgeObjectsToAdd[1]`, named in the edge's objects as -1 (TPF2's
-- tool and scripts did so; INFERRED on TF3); a removed one goes into
-- `edgeObjectsToRemove`. The lane configurations at the edge's ends name it:
-- they are replaced by the same settings naming the rebuilt edge, as for any
-- edge a replay removes (networkInto).

-- The existing edge a stop action names, as edgeBetween finds it.
local function stopEdge(ref)
	local e = edgeBetween(nodesOfNetwork(ref.network), ref.network, arr(ref.ends.a), arr(ref.ends.b))
	if e == nil then error("no " .. ref.network .. " edge for the stop", 0) end
	return e
end

-- The edge `id` rebuilt as `s` keeps its owner: its PlayerOwned is a
-- component of its own, which the edge's BASE_EDGE does not carry, and
-- without it the rebuilt edge would be no one's (a company's road
-- everyone's).
local function keepOwner(s, id)
	local owner = require_companies().ownerOf(api, id)
	if owner ~= nil then
		local ok = pcall(function() s.playerOwned.player = owner end)
		if not ok then
			local owned = api.type.PlayerOwned.new()
			owned.player = owner
			s.playerOwned = owned
		end
	end
end

-- A proposal that removes edge `e` and adds it again with `objects`.
local function rebuildWith(e, network, objects)
	local proposal = api.type.SimpleProposal.new()
	local s = api.type.SegmentAndEntity.new()
	s.entity = -1
	s.comp = api.engine.getComponent(e.id, api.type.ComponentType.BASE_EDGE)
	s.type = network == "Track" and 1 or 0
	s.comp.objects = objects
	keepOwner(s, e.id)
	proposal.streetProposal.edgesToAdd = { s }
	proposal.streetProposal.edgesToRemove = { e.id }
	-- The lane configurations at its ends name the edge, so they go and come
	-- back naming the rebuilt one, their turns, crosswalks and lights as they
	-- were (junctions.renamed); one that cannot fails the stop in every game.
	-- Removed alone, a junction with traffic lights kept its lights with no
	-- configuration: a fatal assert in every game (build 40408, 2026-10-04: a
	-- stop on a town road between two traffic lights crashed a room,
	-- ecs::Engine::GetComponentDataIndex, BaseNodeConfig).
	junctions.renamed(proposal, { e.comp.node0, e.comp.node1 }, e.id, -1, s.comp)
	return proposal
end

-- A stop the room placed is the acting company's, the same in every game
-- (2026-10-02: a company's stops came out another company's, and its
-- player could not open them). The stop's edge object is named for it
-- (`playerEntity`), but what the game's windows and its line manager ask
-- is the owner of the stop's station group (gui/entity_window/
-- station_group.tl). A street stop's edge object is its station itself
-- (mission/name_util.tl: an entity with EDGE_OBJECT and STATION), and its
-- group is the station group system's (getStationGroup of the object); a
-- stop built as a construction has its stations in the construction. So,
-- once built, each new object on the edge, its station group, and any
-- construction with its stations and their groups are given to the acting
-- company where they are anyone else's, as the game's own missions hand a
-- stop over (transfer_ownership_util.tl, makeEntitySetPlayerCmd). A
-- station group that holds a station of another stop is left as it is: it
-- is not this stop's to give. hook.log says, for each new object, whether
-- it is a station and which group holds it (2026-10-02, retest: the hand-
-- over found the objects alone, and no icon showed). `kept` are the
-- edge's objects as the proposal listed them, the new ones negative.
local function settleStop(ref, kept, model, name)
	local ok, e = pcall(stopEdge, ref)
	if not ok then
		log("the new " .. tostring(model) .. ": its edge cannot be found again to settle its owner")
		return
	end
	local had = {}
	for _, o in ipairs(kept) do
		if o[1] >= 0 then had[o[1]] = true end
	end
	local me = company()
	local companies = require_companies()
	local C = api.type.ComponentType
	local seen, fixed, found = {}, {}, {}
	local function give(entity, what)
		if type(entity) ~= "number" or entity < 0 or seen[entity] then return end
		seen[entity] = true
		local owner = companies.ownerOf(api, entity)
		if owner == me then return end
		-- The stop stands either way: a refusal is logged, not the action's.
		local sent, why = pcall(function() send(api.cmd.makeEntitySetPlayerCmd(entity, me)) end)
		fixed[#fixed + 1] = what .. " " .. entity .. " (was " .. tostring(owner) .. ")"
			.. (sent and "" or (": refused, " .. tostring(why)))
	end
	local function groupOf(station)
		local group = -1
		pcall(function() group = api.engine.system.stationGroupSystem.getStationGroup(station) end)
		if type(group) ~= "number" or group < 0 then return nil end
		return group
	end
	-- The new objects, and every station of this stop: what its groups may
	-- hold and still be its own.
	local objects, stations, mine, conOf = {}, {}, {}, {}
	for _, o in ipairs(e.comp.objects or {}) do
		if not had[o[1]] then
			objects[#objects + 1] = o[1]
			mine[o[1]] = true
		end
	end
	for _, object in ipairs(objects) do
		local isStation = false
		pcall(function() isStation = api.engine.getComponent(object, C.STATION) ~= nil end)
		local con = -1
		pcall(function() con = api.engine.util.construction.getConstructionEntity(object) end)
		if type(con) ~= "number" then con = -1 end
		local group = groupOf(object)
		found[#found + 1] = tostring(object) .. (isStation and " a station" or " no station")
			.. (group and (" in group " .. group .. " (owner " .. tostring(companies.ownerOf(api, group)) .. ")")
				or " in no group")
			.. (con >= 0 and (", construction " .. con) or "")
		if isStation or group then stations[#stations + 1] = object end
		if con >= 0 then
			local c = api.engine.getComponent(con, C.CONSTRUCTION)
			for _, s in ipairs(c and c.stations or {}) do
				stations[#stations + 1] = s
				mine[s] = true
			end
			conOf[object] = con
		end
	end
	for _, object in ipairs(objects) do
		give(object, "stop")
		if conOf[object] then give(conOf[object], "construction") end
	end
	local own, byGroup = {}, {}
	for _, s in ipairs(stations) do
		give(s, "station")
		local group = groupOf(s)
		local g = group and api.engine.getComponent(group, C.STATION_GROUP)
		local alone = g ~= nil and g ~= false
		for _, other in ipairs(g and g.stations or {}) do
			if not mine[other] then alone = false end
		end
		if alone then
			give(group, "station group")
			if not byGroup[group] then
				byGroup[group] = { group = group, stations = {} }
				own[#own + 1] = byGroup[group]
			end
			table.insert(byGroup[group].stations, s)
		end
	end
	log("the new " .. tostring(model) .. ": " .. table.concat(found, "; "))
	if #fixed > 0 then
		log("the new " .. tostring(model) .. " made the acting company's (" .. tostring(me) .. "): "
			.. table.concat(fixed, ", "))
	end
	-- Named after its town, as nameStationGroups names a stop's group.
	-- By the tool's name where the stop carries one (only where the game
	-- left its group unnamed), else after its town.
	nameStationGroups(own, name, PROVISIONAL_STOP_NAME)
end

-- How near its edge's centreline a stop's place is: the originator's own
-- point of that centreline, rounded to the millimetre.
local STOP_TOLERANCE = 0.5

-- The entity a proposal gives its first new edge object (build 40408).
local NEW_EDGE_OBJECT = -400000000

function HANDLERS.PlaceStop(stop)
	local network = stop.edge.network
	-- The tool's own name for it, as every game received it.
	local native = apply.NATIVE_STOP_NAMES and type(stop.name) == "string" and stop.name ~= "" and stop.name or nil
	local e = stopEdge(stop.edge)
	local u, off = geom.parameterAt(e.a, e.ta, e.b, e.tb, stop.at.x, stop.at.y)
	if off > STOP_TOLERANCE then error("the stop's place is not on its edge", 0) end
	-- The engine's side, flipped where this edge runs the other way.
	local left = stop.left == true
	local t, d = geom.hermiteTangent(e.a, e.ta, e.b, e.tb, u), stop.direction
	if t[1] * d.x + t[2] * d.y + t[3] * d.z < 0 then left = not left end
	local types = enum("EdgeObjectType")
	local isStop = stop.object == nil or stop.object == "Stop"
	local function typeOf(l)
		if not isStop then return types.SIGNAL end
		return l and types.STOP_LEFT or types.STOP_RIGHT
	end
	-- The sides it takes: one, or both for a two-sided stop, the
	-- originator's first side first, as its tool added them.
	local sides = { left }
	if isStop and stop.two_sided == true then sides[2] = not left end
	-- One stop a side: a second is a fatal assert in the game's lane
	-- creation (TPF2, docs/BUILDING.md). Signals are not by side.
	local objects = {}
	for i, o in ipairs(e.comp.objects or {}) do
		if isStop then
			for _, l in ipairs(sides) do
				if o[2] == typeOf(l) then error("the edge has a stop on that side already", 0) end
			end
		end
		objects[i] = { o[1], o[2] }
	end
	local added = {}
	for k, l in ipairs(sides) do
		-- A new edge object is named by its place in edgeObjectsToAdd,
		-- from -400000000 down (build 40408: con_util_entity_index.h
		-- asserts the range, a fatal error; game_mechanics/towns/
		-- town_util.tl; the stop tool's own proposals).
		objects[#objects + 1] = { NEW_EDGE_OBJECT - (k - 1), typeOf(l) }
		local eo = api.type.SimpleStreetProposal.EdgeObject.new()
		eo.edgeEntity = -1
		eo.param = u
		eo.left = l
		eo.oneWay = stop.one_way == true
		eo.model = stop.model
		eo.playerEntity = company()
		-- A name, so the engine gives the stop its NAME and its owner
		-- (docs/BUILDING.md: an empty name leaves both off); its group is
		-- named after its town once built (settleStop).
		eo.name = native or PROVISIONAL_STOP_NAME
		-- The settings the originator's tool built it with (a mod's on a
		-- signal among them), in every game alike; none, the construction's
		-- defaults (SimpleStreetProposal.EdgeObject.params: not in the
		-- game's API reference, but a member on build 40408, and the signal
		-- it builds keeps them, 2026-10-06).
		if stop.params and #stop.params > 0 then eo.params = objectParams(stop.params) end
		added[k] = eo
	end
	local proposal = rebuildWith(e, network, objects)
	proposal.streetProposal.edgeObjectsToAdd = added
	log(string.format("placing %s on %s edge %d at %.4f, %s", tostring(stop.model), network, e.id, u,
		stop.two_sided == true and "both sides" or (left and "left" or "right")))
	-- Paid by the player, as the tool builds.
	local context = api.type.Context.new()
	context.player = company()
	local built = buildProposal(proposal, context)
	settleStop(stop.edge, objects, stop.model, native)
	return built
end

-- The stops the bulldozer removes from one edge (one, or both of a
-- two-sided stop): for each, the object of its construction on the edge
-- nearest where it stood, within 2 m, each a different one; the edge
-- rebuilt once without them.
function removeEdgeObject(edge, removed, context, twoSided)
	if type(removed) ~= "table" then error("the bulldozer has no edge-object list", 0) end
	local count = 0
	for k in pairs(removed) do
		if type(k) ~= "number" or k ~= math.floor(k) or k < 1 or k > 2 then
			error("the bulldozer has a malformed edge-object list", 0)
		end
		count = count + 1
	end
	local expected = twoSided and 2 or 1
	if count ~= expected or #removed ~= count then
		if twoSided then error("a two-sided stop removal must name both objects", 0) end
		error("a stop removal must name one object", 0)
	end
	if twoSided and removed[1].model ~= removed[2].model then
		error("a two-sided stop removal must name one construction", 0)
	end
	local network = edge.network
	local e = stopEdge(edge)
	local taken, gone, names = {}, {}, {}
	local edgeKinds = {}
	for _, o in ipairs(e.comp.objects or {}) do
		local id, kind = o[1], o[2]
		if type(id) ~= "number" or id ~= math.floor(id) or id < 0 or kind == nil or edgeKinds[id] ~= nil then
			error("the edge has an unreadable or repeated object", 0)
		end
		edgeKinds[id] = kind
	end
	local resolved = {}
	for _, ref in ipairs(removed) do
		local best, bestD, bestKind, bestAt
		for _, o in ipairs(e.comp.objects or {}) do
			local c = not taken[o[1]] and api.engine.getComponent(o[1], api.type.ComponentType.EDGE_OBJECT)
			local t = c and c.transf
			if c and c.edgeObjectConstruction == ref.model and t then
				local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
				local dist = dx * dx + dy * dy + dz * dz
				if dist <= 4 then
					if bestD == nil or dist < bestD then
						best, bestD, bestKind, bestAt = o[1], dist, edgeKinds[o[1]], { t[13], t[14], t[15] }
					elseif dist == bestD then
						error("two " .. tostring(ref.model) .. " objects equally near", 0)
					end
				end
			end
		end
		if best == nil then error("no " .. tostring(ref.model) .. " there", 0) end
		if bestKind == nil then error("an edge object with no stop or signal type", 0) end
		mine(best, "stop")
		taken[best] = true
		gone[#gone + 1] = best
		names[#names + 1] = tostring(ref.model) .. " " .. tostring(best)
		resolved[#resolved + 1] = { entity = best, kind = bestKind, at = bestAt }
	end
	if twoSided then
		local types = enum("EdgeObjectType")
		if resolved[1].kind ~= types.STOP_LEFT and resolved[1].kind ~= types.STOP_RIGHT then
			error("a two-sided stop removal resolved to a signal or waypoint", 0)
		end
		if resolved[2].kind ~= types.STOP_LEFT and resolved[2].kind ~= types.STOP_RIGHT then
			error("a two-sided stop removal resolved to a signal or waypoint", 0)
		end
		if resolved[1].kind == resolved[2].kind or resolved[1].entity == resolved[2].entity then
			error("a two-sided stop removal needs opposite sides", 0)
		end
		local function stationGroup(entity)
			local ok, group = pcall(function()
				return api.engine.system.stationGroupSystem.getStationGroup(entity)
			end)
			if not ok or type(group) ~= "number" or group < 0 then
				error("a two-sided stop without a readable station group", 0)
			end
			local componentType = api.type.ComponentType.STATION_GROUP
			if type(componentType) ~= "number" then error("the station group component cannot be read", 0) end
			local component = api.engine.getComponent(group, componentType)
			if component == nil then error("a two-sided stop with no station group component", 0) end
			local listed = 0
			for _, member in ipairs(seq(component.stations or {})) do
				if member == entity then listed = listed + 1 end
			end
			if listed ~= 1 then error("a two-sided stop not listed once in its station group", 0) end
			return group
		end
		if stationGroup(resolved[1].entity) ~= stationGroup(resolved[2].entity) then
			error("the two stops are not one two-sided stop", 0)
		end
		local function center(at)
			local u = geom.parameterAt(e.a, e.ta, e.b, e.tb, at[1], at[2])
			return geom.hermitePos(e.a, e.ta, e.b, e.tb, u)
		end
		local a, b = center(resolved[1].at), center(resolved[2].at)
		local dx, dy, dz = a[1] - b[1], a[2] - b[2], a[3] - b[3]
		if dx * dx + dy * dy + dz * dz > 0.25 then
			error("the two stops are at different places on the edge", 0)
		end
	end
	local objects = {}
	for _, o in ipairs(e.comp.objects) do
		if not taken[o[1]] then objects[#objects + 1] = { o[1], o[2] } end
	end
	local proposal = rebuildWith(e, network, objects)
	proposal.streetProposal.edgeObjectsToRemove = gone
	log("removing " .. table.concat(names, ", ") .. " from " .. network .. " edge " .. e.id)
	return buildProposal(proposal, context)
end

-- How near each end of a track a PlaceSignals names is its node, in three
-- dimensions: the node's own position, rounded to the millimetre.
local SIGNAL_END_TOLERANCE = 0.5
-- How near where a removed signal stood, along its track, the signal of its
-- construction there is.
local SIGNAL_TOLERANCE = 0.25

-- The track between the nodes at `a` and `b` (an action's ends), as it
-- stands: its id, component and whether it runs from `a` to `b`. One track
-- node within SIGNAL_END_TOLERANCE of each end, in three dimensions (a
-- track may pass over another), and one track between them; two of either
-- refuse it, never chosen between by this game's own entities (D8).
local function signalTrack(nodes, a, b)
	local function only(p)
		local found
		for _, n in ipairs(nodes) do
			local dx, dy, dz = n.pos[1] - p.x, n.pos[2] - p.y, (n.pos[3] or 0) - (p.z or 0)
			if dx * dx + dy * dy + dz * dz <= SIGNAL_END_TOLERANCE * SIGNAL_END_TOLERANCE then
				if found ~= nil then error("two track nodes where a signal's track ends", 0) end
				found = n
			end
		end
		if found == nil then error("no track node where a signal's track ends", 0) end
		return found
	end
	local na, nb = only(a), only(b)
	if na.id == nb.id then error("a signal's track from a node to itself", 0) end
	local ids = api.engine.system.streetSystem.getNodeTrackSegments(na.id)
	local found
	for i = 1, (ids and #ids or 0) do
		local c = api.engine.getComponent(ids[i], api.type.ComponentType.BASE_EDGE)
		if c and ((c.node0 == na.id and c.node1 == nb.id) or (c.node0 == nb.id and c.node1 == na.id)) then
			if found ~= nil then error("two tracks between a signal's track's ends", 0) end
			local forward = c.node0 == na.id
			found = { id = ids[i], comp = c, forward = forward,
				p0 = forward and na.pos or nb.pos, p1 = forward and nb.pos or na.pos,
				t0 = arr(c.tangent0), t1 = arr(c.tangent1) }
		end
	end
	if found == nil then error("no track between a signal's track's ends", 0) end
	return found
end

-- A track's length along its curve, in metres (its Hermite curve in 32
-- steps: near enough for SIGNAL_TOLERANCE on the tracks a game draws).
local function trackLength(t)
	local length, last = 0, t.p0
	for i = 1, 32 do
		local p = geom.hermitePos(t.p0, t.t0, t.p1, t.t1, i / 32)
		local dx, dy, dz = p[1] - last[1], p[2] - last[2], (p[3] or 0) - (last[3] or 0)
		length = length + math.sqrt(dx * dx + dy * dy + dz * dz)
		last = p
	end
	return length
end

-- The signals a PlaceSignals placed are the acting company's, the same in
-- every game, as a stop's are (settleStop): each track found again by its
-- ends, and each object on it that it did not have before given to the
-- company where it is anyone else's.
local function settleSignals(tracks, sig, me)
	local nodes = readNodes("Track")
	local companies = require_companies()
	local given = 0
	for i, entry in ipairs(sig.edges) do
		local had = {}
		for _, o in ipairs(tracks[i].comp.objects or {}) do had[o[1]] = true end
		local ok, t = pcall(signalTrack, nodes, entry.edge.a, entry.edge.b)
		if not ok then
			log("the new signals: a track cannot be found again to settle their owner: " .. tostring(t))
		else
			for _, o in ipairs(t.comp.objects or {}) do
				if not had[o[1]] and companies.ownerOf(api, o[1]) ~= me then
					local sent = pcall(function() send(api.cmd.makeEntitySetPlayerCmd(o[1], me)) end)
					if sent then given = given + 1 else log("the new signal " .. tostring(o[1]) .. ": its owner was not set") end
				end
			end
		end
	end
	if given > 0 then log("the new signals made the acting company's (" .. tostring(me) .. "): " .. given) end
end

-- Signals added to and removed from existing tracks (action::PlaceSignals:
-- what Auto Signals builds after its player's signal), in one proposal:
-- every track named is rebuilt between its nodes with its other objects
-- kept under their own entities, as a stop is placed. Every track and every
-- removed signal is found first, in the world as it stands, one for one;
-- anything missing or ambiguous refuses the whole build, in every game
-- alike. A place along a track is from its `a` end: where this game's track
-- runs from `b`, it is 1 - place there, and the side flips.
function HANDLERS.PlaceSignals(sig)
	local C = api.type.ComponentType
	local SIGNAL = enum("EdgeObjectType").SIGNAL
	local nodes = readNodes("Track")
	local tracks, seen = {}, {}
	for i, entry in ipairs(sig.edges) do
		local t = signalTrack(nodes, entry.edge.a, entry.edge.b)
		if seen[t.id] then error("a signal build that names a track twice", 0) end
		seen[t.id] = true
		tracks[i] = t
	end
	local me = company()
	local segs, removed, added, gone, renames, ends = {}, {}, {}, {}, {}, {}
	for i, entry in ipairs(sig.edges) do
		local t = tracks[i]
		local function here(at) if t.forward then return at end return 1 - at end
		local taken = {}
		local length
		for _, old in ipairs(entry.remove or {}) do
			length = length or trackLength(t)
			local want, best = here(old.at), nil
			for _, o in ipairs(t.comp.objects or {}) do
				local c = o[2] == SIGNAL and not taken[o[1]] and api.engine.getComponent(o[1], C.EDGE_OBJECT)
				if c and c.edgeObjectConstruction == old.model and math.abs(c.param - want) * length <= SIGNAL_TOLERANCE then
					if best ~= nil then error("two signals where one is removed", 0) end
					best = o[1]
				end
			end
			if best == nil then error("no " .. tostring(old.model) .. " where a signal is removed", 0) end
			mine(best, "signal")
			taken[best] = true
			gone[#gone + 1] = best
		end
		local objects = {}
		for _, o in ipairs(t.comp.objects or {}) do
			if not taken[o[1]] then objects[#objects + 1] = { o[1], o[2] } end
		end
		for _, new in ipairs(entry.add or {}) do
			local k = #added + 1
			objects[#objects + 1] = { NEW_EDGE_OBJECT - (k - 1), SIGNAL }
			local eo = api.type.SimpleStreetProposal.EdgeObject.new()
			eo.edgeEntity = -i
			eo.param = here(new.at)
			eo.left = (new.left == true) == t.forward
			eo.oneWay = sig.one_way == true
			eo.model = sig.model
			eo.playerEntity = me
			-- Named, as a stop the room places is, so the engine gives it
			-- its owner (docs/BUILDING.md).
			eo.name = PROVISIONAL_STOP_NAME
			if sig.params and #sig.params > 0 then eo.params = objectParams(sig.params) end
			added[k] = eo
		end
		local s = api.type.SegmentAndEntity.new()
		s.entity = -i
		s.comp = api.engine.getComponent(t.id, C.BASE_EDGE)
		s.type = 1
		s.comp.objects = objects
		keepOwner(s, t.id)
		segs[i] = s
		removed[i] = t.id
		renames[t.id] = { id = -i, comp = s.comp }
		ends[#ends + 1] = t.comp.node0
		ends[#ends + 1] = t.comp.node1
	end
	local proposal = api.type.SimpleProposal.new()
	proposal.streetProposal.edgesToAdd = segs
	proposal.streetProposal.edgesToRemove = removed
	proposal.streetProposal.edgeObjectsToAdd = added
	if #gone > 0 then proposal.streetProposal.edgeObjectsToRemove = gone end
	-- The lane configurations at the tracks' ends name them: they go and
	-- come back naming the rebuilt tracks, all in one (junctions.renamedAll).
	junctions.renamedAll(proposal, ends, renames)
	log(string.format("placing %d and removing %d signals on %d tracks", #added, #gone, #segs))
	local context = api.type.Context.new()
	context.player = me
	local built = buildProposal(proposal, context)
	settleSignals(tracks, sig, me)
	return built
end

-- ------------------------------------------------------ vehicles and lines
--
-- Vehicles, lines and station groups are named by canonical id
-- (tpf3mp/registry.lua): `ctx.registry` is the game script's, up to date.
-- A depot is named by its construction's file and position.

local registry = module("registry")
local captureModule = module("capture")
local companiesModule = module("companies")
require_companies = function() return companiesModule end
local progressionModule = module("progression")

local function entityOf(ctx, kind, id)
	local e = registry.entity(ctx and ctx.registry, kind, id)
	if e == nil then error("no " .. kind .. " " .. tostring(id) .. " in this world", 0) end
	return e
end

-- As entityOf, for one the acting company must own: its own vehicles and
-- lines, never another company's.
local OWNED_WHAT = { vehicles = "vehicle", lines = "line" }
local function ownOf(ctx, kind, id)
	local e = entityOf(ctx, kind, id)
	mine(e, OWNED_WHAT[kind] or kind)
	return e
end

-- The construction of `ref.file` whose origin is within 2 m of `ref.at`, the
-- nearest, the lower entity on a tie.
function constructionAt(ref)
	local t0 = costs and clock()
	local CONSTRUCTION = api.type.ComponentType.CONSTRUCTION
	local list = api.engine.getEntitiesWithComponent(CONSTRUCTION)
	local best, bestD
	for i = 1, #list do
		local e = list[i]
		local c = api.engine.getComponent(e, CONSTRUCTION)
		if c and c.fileName == ref.file then
			local t = c.transf
			local dx, dy, dz = t[13] - ref.at.x, t[14] - ref.at.y, t[15] - ref.at.z
			local d = dx * dx + dy * dy + dz * dz
			if d <= 4 and (bestD == nil or d < bestD or (d == bestD and e < best)) then best, bestD = e, d end
		end
	end
	spend("constructionAt", t0, #list)
	if best == nil then error("no " .. tostring(ref.file) .. " there", 0) end
	return best, api.engine.getComponent(best, CONSTRUCTION)
end

-- One channel of a colour as the room carries it, in millionths, put back
-- on the game's 1/255 steps when it was on one. The game's colours are
-- steps (gui/main/color_internal.lua: channel / 255), and its line manager
-- tells which palette colour a line wears by floor(channel * 255)
-- (line_vehicle_mgmt/line_util.tl, calcColorKey): 127/255 carried as
-- 0.498039 is step 126 there, so a new line never counted against the
-- colour it took, and every next line took that colour again. The
-- millionths are at most 0.00013 of a step off; a channel further from a
-- step is not one, and stays as carried. Plain arithmetic, the same in
-- every game.
local function channel(v)
	local steps = v * 255
	local step = math.floor(steps + 0.5)
	if math.abs(steps - step) < 0.001 then return step / 255 end
	return v
end

local function tint(c) return api.type.Vec3f.new(channel(c.r), channel(c.g), channel(c.b)) end

-- The game's time here, the same in every game: when a part is bought.
local function now()
	return api.engine.getComponent(api.engine.util.getWorld(), api.type.ComponentType.GAME_TIME).gameTime
end

-- A ConsistPart as the game's TransportVehiclePart, bought at `time`. Every
-- compartment loads automatically, as the store sends it
-- (vehicle_react_util.tl).
-- How many compartments a vehicle model has (its transportVehicle
-- metadata), or nil where the game does not say.
function apply.compartments(model)
	local ok, n = pcall(function()
		local tv = api.res.modelRep.get(model).metadata.transportVehicle
		local count = 0
		for _ in ipairs(tv.compartments) do count = count + 1 end
		return count
	end)
	if ok and type(n) == "number" then return n end
	return nil
end

local function vehiclePart(p, time)
	local model = api.res.modelRep.find(p.model)
	if type(model) ~= "number" or model < 0 then error("no vehicle model " .. tostring(p.model), 0) end
	local part = api.type.TransportVehiclePart.new()
	part.part.modelId = model
	part.part.reversed = p.reversed == true
	local loads, auto = {}, {}
	for k, l in ipairs(p.loads) do
		local lc = api.type.LoadConfig.new()
		lc.loadConfigIndex = l.config
		lc.cargoTypeId = l.cargo
		loads[k], auto[k] = lc, true
	end
	-- The game takes a load for every compartment of the model, and throws
	-- ("Unknown exception", build 40408) for a part with fewer. A part that
	-- names none gets the store's own: the first load configuration of each
	-- compartment (gui/line_vehicle_mgmt/vehicle_util.tl); one that names
	-- some but not all is refused, the same in every game.
	local compartments = apply.compartments(model)
	if compartments ~= nil and #loads ~= compartments then
		if #loads > 0 then
			error(string.format("%s has %d compartments, and the part loads %d", tostring(p.model), compartments,
				#loads), 0)
		end
		for k = 1, compartments do
			local lc = api.type.LoadConfig.new()
			lc.loadConfigIndex = 0
			loads[k], auto[k] = lc, true
		end
	end
	part.part.compartment2loadConfig = loads
	part.part.color = tint(p.color)
	part.purchaseTime = time
	part.autoLoadConfig = auto
	return part
end

-- A TransportVehicleConfig of these parts, groups and multiple units.
local function vehicleConfig(vehicles, groups, units)
	local config = api.type.TransportVehicleConfig.new()
	config.vehicles = vehicles
	config.vehicleGroups = seq(groups)
	config.muFileNames = seq(units)
	return config
end

-- The depot a purchase names: its construction's depot by its index there,
-- among the construction's depots as the capture listed them
-- (capture.depotsOf: its `depots`, then its subconstructions that are
-- depots, such as an airfield's or airport's hangar). One the construction
-- does not have is refused, the same in every game, saying how many it has;
-- never another depot of it.
local function purchaseDepot(buy)
	local _, construction = constructionAt(buy.depot)
	local depots = captureModule.depotsOf(api, construction)
	local index = (buy.depot_index or 0) + 1
	local depot = depots[index]
	if depot ~= nil then return depot end
	local file = tostring(buy.depot.file)
	if #depots == 0 then
		error(string.format("the %s there has no depot: an airfield or airport has one only with a hangar "
			.. "module, and a harbour never has one (ships are bought at a ship depot)", file), 0)
	end
	error(string.format("the %s there has %d depot(s), and no depot %d", file, #depots, index), 0)
end

function HANDLERS.BuyVehicle(buy)
	local depot = purchaseDepot(buy)
	local allowed, why = companiesModule.mayBuyAtDepot(acting and acting.roster, company(), depot, api)
	if not allowed then error(why, 0) end
	local time = now()
	local vehicles = {}
	for i, p in ipairs(buy.consist) do vehicles[i] = vehiclePart(p, time) end
	local config = vehicleConfig(vehicles, buy.groups, buy.multiple_units)
	-- The game's buy says nothing of why it refused: the likeliest reasons,
	-- the company's money and the depot's room, go with the refusal.
	local sent, data, entities = pcall(send, api.cmd.makeVehicleBuyCmd(company(), depot, config))
	if not sent then
		local facts = {}
		pcall(function()
			local account = api.engine.getComponent(company(), api.type.ComponentType.ACCOUNT)
			facts[#facts + 1] = "the company has " .. string.format("%.0f", account.balance)
		end)
		for i, p in ipairs(buy.consist) do
			pcall(function()
				local model = api.res.modelRep.find(p.model)
				facts[#facts + 1] = string.format("part %d %s: %s compartments, %d loads", i, tostring(p.model),
					tostring(apply.compartments(model)), #p.loads)
			end)
		end
		facts[#facts + 1] = "depot entity " .. tostring(depot)
		pcall(function()
			local d = api.engine.getComponent(depot, api.type.ComponentType.VEHICLE_DEPOT)
			if d and d.vehicles then facts[#facts + 1] = "the depot holds " .. #d.vehicles end
		end)
		error(tostring(data) .. (#facts > 0 and (" (" .. table.concat(facts, "; ") .. ")") or ""), 0)
	end

	local vehicle = madeBy("resultVehicleEntity", data, entities)
	-- With more than one company, in its company's colour.
	local roster = acting and acting.roster
	if vehicle and companiesModule.painting(roster) then
		companiesModule.paintVehicle(companiesModule.byEntity(roster, company()), vehicle, send, api)
	end
	return true, vehicle
end

-- Whether `e` is a vehicle in this world.
local function isVehicle(e)
	if type(e) ~= "number" or e < 0 then return false end
	local ok, c = pcall(api.engine.getComponent, e, api.type.ComponentType.TRANSPORT_VEHICLE)
	if not ok then
		-- The replacement command already ran. Treat an unreadable result
		-- as uncertain, so replay holds before the registry could assign a
		-- new canonical id to the vehicle.
		error({ replayFatal = "replacement vehicle " .. tostring(e) .. " could not be read: " .. tostring(c) }, 0)
	end
	return c ~= nil
end

-- A vehicle's consist replaced, as the store's HandleVehicleChanges sends it
-- (vehicle_react_util.tl): a part the vehicle keeps is its own part, its
-- purchase time and wear as this game has them now, with the facing, loads
-- and colour the player chose; a new part is bought now. The vehicle is
-- then the entity the game names (its command data, its result entities)
-- that is a vehicle, else the vehicle itself: TF3's API says the vehicle is
-- replaced (cmd.d.tl), and its command data has no result field. Returns
-- that entity, for the registry to keep the vehicle's id on.
function HANDLERS.ReplaceVehicle(replace, ctx)
	local vehicle = ownOf(ctx, "vehicles", replace.vehicle)
	local tv = api.engine.getComponent(vehicle, api.type.ComponentType.TRANSPORT_VEHICLE)
	local own = tv and tv.transportVehicleConfig and tv.transportVehicleConfig.vehicles
	if own == nil then error("vehicle " .. tostring(replace.vehicle) .. " has no parts to read", 0) end
	local time = now()
	local vehicles, kept = {}, {}
	for i, r in ipairs(replace.consist) do
		local part = vehiclePart(r.part, time)
		if r.kept ~= nil then
			local old = own[r.kept + 1]
			if old == nil then error("part " .. i .. " keeps a part the vehicle does not have", 0) end
			if old.part.modelId ~= part.part.modelId then
				error("part " .. i .. " keeps a part of another model", 0)
			end
			if kept[r.kept] then error("part " .. i .. " keeps a part kept already", 0) end
			kept[r.kept] = true
			part.purchaseTime = old.purchaseTime
			part.maintenanceState = old.maintenanceState
			part.maintenanceChange = old.maintenanceChange
		end
		vehicles[i] = part
	end
	local config = vehicleConfig(vehicles, replace.groups, replace.multiple_units)
	local count = 0
	for _ in pairs(kept) do count = count + 1 end
	log("replacing vehicle " .. tostring(replace.vehicle) .. " (entity " .. tostring(vehicle) .. "): "
		.. #vehicles .. " part(s), " .. count .. " kept")
	local data, entities = send(api.cmd.makeVehicleReplaceCmd(vehicle, config))
	local candidates = {}
	for _, pair in ipairs(type(entities) == "table" and entities or {}) do
		if type(pair) == "table" then candidates[#candidates + 1] = pair[1] end
	end
	local ok, named = pcall(function() return data.vehicleEntity end)
	if ok then candidates[#candidates + 1] = named end
	candidates[#candidates + 1] = vehicle
	for _, e in ipairs(candidates) do
		if isVehicle(e) then
			if e ~= vehicle then log("vehicle " .. tostring(replace.vehicle) .. " is entity " .. e .. " now, was " .. vehicle) end
			return true, e
		end
	end
	return true, nil
end

function HANDLERS.SellVehicle(sell, ctx)
	local vehicles = {}
	for i, v in ipairs(sell.vehicles) do vehicles[i] = ownOf(ctx, "vehicles", v) end
	return run(api.cmd.makeVehicleSellCmd(vehicles))
end

function HANDLERS.AssignLine(assign, ctx)
	if assign.line == nil then
		return false, "this version of the mod does not take vehicles off their line yet"
	end
	-- No first stop: the game's choice, the next stop each can reach (-1).
	local first = assign.first_stop
	if first == nil then first = -1 end
	local lineOk, line = pcall(ownOf, ctx, "lines", assign.line)
	if not lineOk then
		local lineRead, nativeLine = pcall(entityOf, ctx, "lines", assign.line)
		if not lineRead then nativeLine = nil end
		for _, vehicleId in ipairs(assign.vehicles) do
			local vehicleRead, nativeVehicle = pcall(entityOf, ctx, "vehicles", vehicleId)
			if not vehicleRead then nativeVehicle = nil end
			pcall(diagnosticVehicleAssignment, ctx, vehicleId, nativeVehicle, assign.line, nativeLine, first, "refused")
		end
		error(line, 0)
	end
	for _, v in ipairs(assign.vehicles) do
		local vehicleOk, entity = pcall(ownOf, ctx, "vehicles", v)
		if not vehicleOk then
			local vehicleRead, nativeVehicle = pcall(entityOf, ctx, "vehicles", v)
			if not vehicleRead then nativeVehicle = nil end
			pcall(diagnosticVehicleAssignment, ctx, v, nativeVehicle, assign.line, line, first, "refused")
			error(entity, 0)
		end
		local sent, result = pcall(function()
			return run(api.cmd.makeVehicleSetLineCmd(entity, line, first))
		end)
		pcall(diagnosticVehicleAssignment, ctx, v, entity, assign.line, line, first,
			sent and result == true and "applied" or "refused")
		if not sent then error(result, 0) end
	end
	return true
end

function HANDLERS.VehicleOp(op, ctx)
	local vehicle = ownOf(ctx, "vehicles", op.vehicle)
	local change = op.change
	if type(change) == "table" and change.Stop ~= nil then
		return run(api.cmd.makeVehicleSetStoppedByUserCmd(vehicle, change.Stop == true))
	elseif type(change) == "table" and change.ToDepot then
		-- Sold on arrival, the game crashes at the depot (capture.vehicleToDepot).
		if change.ToDepot.sell == true then
			return false, "selling a vehicle when it reaches the depot (the game crashes there)"
		end
		return run(api.cmd.makeVehicleSendToDepotCmd(vehicle, false))
	elseif change == "Reverse" then
		return run(api.cmd.makeVehicleReverseCmd(vehicle))
	elseif change == "Depart" then
		return run(api.cmd.makeVehicleTryToDepartCmd(vehicle))
	elseif type(change) == "table" and change.ManualDeparture ~= nil then
		return run(api.cmd.makeVehicleSetManualDepartureCmd(vehicle, change.ManualDeparture == true))
	elseif type(change) == "table" and change.Recolor then
		return run(api.cmd.makeEntitySetColorCmd(vehicle, tint(change.Recolor)))
	end
	return false, "a vehicle change of no kind"
end

-- Renaming a vehicle, a station, a town or a construction (action::Renamed),
-- as its window does: the acting company's own vehicle, a station or
-- construction no other company owns, any town.
function HANDLERS.Rename(r, ctx)
	local what, e = r.what, nil
	if type(what) ~= "table" then return false, "renaming nothing" end
	if what.Vehicle ~= nil then
		e = ownOf(ctx, "vehicles", what.Vehicle)
	elseif what.Station ~= nil then
		e = entityOf(ctx, "groups", what.Station)
		mine(e, "station")
	elseif what.Town ~= nil then
		e = entityOf(ctx, "towns", what.Town)
	elseif what.Construction ~= nil then
		e = constructionAt(what.Construction)
		mine(e, "construction")
	else
		return false, "renaming nothing"
	end
	return run(api.cmd.makeEntitySetNameCmd(e, r.name))
end

-- The game's load modes, by the schema's names, as numbers.
local LOAD_MODES = { LoadIfAvailable = 0, FullLoadAny = 1, FullLoadAll = 2, LegacyUnloadOnly = 3 }

-- A line's waypoint (action::Waypoint) as the game's Waypoint: on a lane,
-- the edge found by its ends (the lowest entity between those nodes, as for
-- a stop), which must run node 0 to node 1 as on the originator's (else
-- the lane's index and place would name another: refused), or the
-- construction at its place; in the open, its position. Its tag as carried.
local function waypointFor(w)
	local wp = api.type.Waypoint.new()
	local at = w.at or {}
	if at.Open then
		wp.pos = api.type.Vec3f.new(at.Open.x, at.Open.y, at.Open.z)
	elseif at.Lane then
		local lane, entity = at.Lane, nil
		if lane.of.Edge then
			local ref = lane.of.Edge
			local nodes = readNodes(ref.network)
			local e = edgeBetween(nodes, ref.network, arr(ref.ends.a), arr(ref.ends.b))
			if e == nil then error("no " .. ref.network .. " edge for a waypoint", 0) end
			local a = nearest(nodes, arr(ref.ends.a), END_TOLERANCE)
			if a == nil or e.node0 ~= a.id then error("a waypoint's edge runs the other way here", 0) end
			entity = e.id
		elseif lane.of.Construction then
			entity = constructionAt(lane.of.Construction)
		else
			error("a waypoint on no network", 0)
		end
		local edgePos = api.type.EdgePos.new()
		edgePos.edgeId = api.type.EdgeId.new(entity, lane.index)
		edgePos.param = lane.param
		wp.edgePos = edgePos
	else
		error("a waypoint with no place", 0)
	end
	wp.tag = w.tag
	return wp
end

-- A LineData as the game's Line component. Each stop is at a station the
-- acting company may use (tpf3mp/companies.lua, mayUse): no company's, its
-- own, or another company's that keeps its stations open (DECISIONS.md, D22,
-- proposed). The game itself stops a line anywhere (build 40408: no owner
-- check on a line's stops); its line manager offers only the player's own
-- stations, which the GUI lifts for open ones (gui/tpf3mp/tpf3mp.script.lua).
local function lineComponent(data, ctx)
	local line = api.type.Line.new()
	local stops = {}
	for i, s in ipairs(data.stops) do
		local stop = api.type.Line.Stop.new()
		local group = entityOf(ctx, "groups", s.group)
		local usable, why = companiesModule.mayUse(ctx and ctx.roster, company(), group, api)
		if not usable then error("stop " .. i .. ": " .. why, 0) end
		stop.stationGroup = group
		stop.station = s.terminal.station
		stop.terminal = s.terminal.terminal
		local alternatives = {}
		for k, a in ipairs(s.alternatives) do
			alternatives[k] = api.type.StationTerminal.new(a.station, a.terminal)
		end
		stop.alternativeTerminals = alternatives
		stop.loadMode = LOAD_MODES[s.load_mode] or error("a load mode " .. tostring(s.load_mode), 0)
		stop.minWaitingTime = s.min_wait
		stop.maxWaitingTime = s.max_wait
		stop.maxAdditionalWaitingTime = s.max_extra_wait
		local config = api.type.Line.StopConfig.new()
		config.load = seq(s.rules.load)
		config.maxLoad = seq(s.rules.max_load)
		config.forceUnload = s.rules.force_unload == true
		config.destroyForConfigChange = s.rules.destroy_for_config_change == true
		config.destroyForRefresh = s.rules.destroy_for_refresh == true
		stop.stopConfig = config
		local waypoints = {}
		for k, w in ipairs(s.waypoints or {}) do waypoints[k] = waypointFor(w) end
		stop.waypoints = waypoints
		stops[i] = stop
	end
	line.stops = stops
	local modes = {}
	for _, m in ipairs(data.modes) do modes[m] = true end
	line.vehicleInfo.transportModes = modes
	line.customFilters = data.custom_filters == true
	line.reservationPriority = data.reservation_priority
	return line
end

function HANDLERS.CreateLine(create, ctx)
	local line = lineComponent(create.line, ctx)
	local data, entities = send(api.cmd.makeLineCreateCmd(create.name, tint(create.color),
		company(), line))
	return true, madeBy("resultEntity", data, entities)
end

function HANDLERS.EditLine(edit, ctx)
	local line = ownOf(ctx, "lines", edit.line)
	local change = edit.change
	if change == "Delete" then
		return run(api.cmd.makeLineDestroyCmd(line))
	elseif type(change) == "table" and change.Update then
		return run(api.cmd.makeLineUpdateCmd(line, lineComponent(change.Update, ctx)))
	elseif type(change) == "table" and change.Rename then
		return run(api.cmd.makeEntitySetNameCmd(line, change.Rename))
	elseif type(change) == "table" and change.Recolor then
		return run(api.cmd.makeEntitySetColorCmd(line, tint(change.Recolor)))
	end
	return false, "a line change of no kind"
end

-- What an action makes (the new vehicle, the new line): its kind in the
-- registry, which the game script binds it in after the action. Its
-- handler returns the entity, where the game said which.
apply.CREATES = { BuyVehicle = "vehicles", CreateLine = "lines" }

-- What an action changes and names by canonical id, which keeps its id
-- whatever entity it is after: the replaced vehicle. Its kind in the
-- registry and the field of the action naming it; its handler returns the
-- entity it is now, where this game could name it.
apply.KEEPS = { ReplaceVehicle = { kind = "vehicles", field = "vehicle" } }

-- A loan's terms as the loan script keeps them (loan.d.tl): the action's
-- table has the script's own field names and fractions.
local function loanTerms(terms)
	local out = {}
	for key, value in pairs(terms) do out[key] = value end
	return out
end

-- Loans go through the loan script's own events, with the parameters the
-- game's finance window sends (game_mechanics/finance/finances_loan_gui.tl):
-- here they run at once, in every game at the same update.
function HANDLERS.Loan(op, ctx)
	-- Another company's loans are the room's (tpf3mp/companies.lua): on the
	-- terms the game offers, booked to that company. Which company is the
	-- roster's to say (company 0 is the save's own player), not
	-- getPlayer()'s, which a GUI state answers with the player's company.
	local roster = ctx and ctx.roster
	local mine = roster and companiesModule.byEntity(roster, company())
	if roster and not mine then return false, "the acting company is not in the room's roster" end
	if mine and mine.id ~= 0 then
		if op.Take then return companiesModule.borrow(roster, mine.id, op.Take.offer, op.Take.next, send, api) end
		if op.Repay then return companiesModule.repay(roster, mine.id, op.Repay.loan, send, api) end
		return false, "a loan is taken or paid back"
	end
	if op.Take then
		return run(api.cmd.makeScriptingSendEventCmd("", "Loan", "Obtain",
			{ loanTerms(op.Take.next), loanTerms(op.Take.offer) }))
	elseif op.Repay then
		local param = {}
		param[2] = loanTerms(op.Repay.loan)
		return run(api.cmd.makeScriptingSendEventCmd("", "Loan", "Repay", param))
	end
	return false, "a loan is taken or paid back"
end

-- Answering a subsidy offer (tpf3mp/companies.lua, "subsidies"): the
-- subsidy script's own events, as the game's subsidy window sends them
-- (game_mechanics/subventions/subventions_gui.tl), here at once, in every
-- game at the same update. Every game checks the offer against its own
-- script's state first, alike: the first company in the room's order to
-- accept an offer takes it, and every later one is refused, naming who took
-- it. The money goes to the acting player's company. The accept carries
-- that company's player entity (`tpf3mpCompany`), which the subsidy's kind
-- keeps as its taker: from then on only the taker's transport counts
-- towards it (tpf3mp/subsidies.lua).
function HANDLERS.Subsidy(op, ctx)
	local roster = ctx and ctx.roster
	if not roster then return false, "no roster to book the subsidy to" end
	local mine = companiesModule.byEntity(roster, company())
	if not mine then return false, "the acting company is not in the room's roster" end
	local state = companiesModule.subsidyState(api)
	local function event(name, ref)
		return function()
			local param = { uid = ref.uid }
			if name == "onAccept" then param.tpf3mpCompany = mine.entity end
			send(api.cmd.makeScriptingSendEventCmd("", "Subvention", name, param))
		end
	end
	if op.Accept then
		local ok, why = companiesModule.acceptSubsidy(roster, mine.id, op.Accept, state,
			event("onAccept", op.Accept), send, api)
		if not ok then return false, why end
		log("subsidy " .. string.format("%.0f", op.Accept.uid) .. " (" .. tostring(op.Accept.kind) .. ") taken by "
			.. tostring(mine.name))
		return true
	elseif op.Decline then
		return companiesModule.declineSubsidy(roster, op.Decline, state, event("onDecline", op.Decline))
	end
	return false, "a subsidy is accepted or declined"
end

-- A notification's popup played its first sound: the game's Notifications
-- script's own event marks it (game_mechanics/notifications/
-- notifications.script.tl, "initialSound"), in every game, so no game
-- plays it again.
function HANDLERS.NotificationSeen(n)
	if type(n.notification) ~= "number" then error("a notification by its id", 0) end
	return run(api.cmd.makeScriptingSendEventCmd("", "Notifications", "initialSound",
		{ notificationId = n.notification }))
end

-- Prospecting goes through the company script's own event, with the
-- parameters the construction menu sends it (gui/construction/
-- construction_react_util.tl): here it runs at once, in every game at the
-- same update, so every game's company script keeps the same prospection
-- from the same game time, and months later draws the same outcome and
-- builds the same industry at the same place: it seeds its draws, and the
-- game its placement, from the game time
-- (investigation/TPF3_PROSPECTING_2026-09-30.md). The company is the
-- player's, as the menu names it; the industry types go in the order the
-- originator's menu listed them.
function HANDLERS.Prospect(p, ctx)
	local town = entityOf(ctx, "towns", p.town)
	local types = seq(p.industries)
	if #types == 0 then error("a prospection that can find no industry", 0) end
	log("prospecting for " .. tostring(p.cargo) .. " near town-" .. tostring(p.town) .. " (" .. tostring(town)
		.. "): " .. table.concat(types, ", "))
	return run(api.cmd.makeScriptingSendEventCmd("", "Companies", "spawnIndustry", {
		companyEntity = company(),
		townEntity = town,
		types = types,
		permitKey = p.permit,
		cargoType = p.cargo,
	}))
end

-- A company perk (action::PerkOp) goes through the company script's own
-- event, with the parameters the construction menu's perk tool sends it
-- (gui/construction/tools/industry_greenify_tool.script.tl,
-- marketing_campaign_tool.script.tl): here it runs at once, in every game
-- at the same update, for the acting company, which spends the permit. The
-- company script hands it on to the emissions or towns script, in this game
-- alone, as in every other.

function HANDLERS.Perk(op, ctx)
	if op.Greenify then
		local g = op.Greenify
		local con = entityOf(ctx, "industries", g.industry)
		local part = captureModule.industryPart(con)
		if part == nil then error("industry-" .. tostring(g.industry) .. " is no one industry here", 0) end
		log("greenifying industry-" .. tostring(g.industry) .. " (" .. tostring(part) .. ")")
		return run(api.cmd.makeScriptingSendEventCmd("", "Companies", "MakeGreen", {
			companyEntity = company(),
			constructionEntity = part,
			permitKey = g.permit,
		}))
	elseif op.Marketing then
		local m = op.Marketing
		local town = entityOf(ctx, "towns", m.town)
		local cost = tonumber(m.cost)
		if cost == nil or cost < 0 then error("a campaign of no price", 0) end
		-- The tool will not start one the company cannot pay for; neither
		-- does any game.
		local read, balance = pcall(function() return api.engine.util.finance.getPlayersBalance(company()) end)
		if not read or type(balance) ~= "number" or balance ~= balance or math.abs(balance) == math.huge then
			error("cannot read the company balance for the campaign", 0)
		end
		if balance < cost then
			error("not enough money for the campaign", 0)
		end
		log("marketing in town-" .. tostring(m.town) .. " (" .. tostring(town) .. ") for " .. tostring(cost))
		send(api.cmd.makeScriptingSendEventCmd("", "Companies", "startMarketingCampaign", {
			townEntity = town,
			companyEntity = company(),
			marketingParams = { durationMs = m.duration_ms, lineCostFactor = m.line_cost_factor },
			permitKey = m.permit,
		}))
		-- What the tool books once the campaign started (its command's
		-- callback): the price, to the company, as another expense.
		local entry = api.type.JournalEntry.new()
		entry.amount = -cost
		entry.time = -1
		entry.category.type = api.type.JournalEntry.Type.OTHER
		return run(api.cmd.makeJournalBookAssetCmd(company(), entry, api.type.Vec3f.new(0, 0, 0)))
	end
	return false, "a perk of no kind"
end

-- The shared date pace; zero holds the date without stopping vehicles.
function HANDLERS.CalendarSpeed(p)
	local value = p.millis_per_day
	-- Also validate direct Lua replay, before constructing an engine command.
	local capture = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/capture.lua")
		or require("tpf3mp.capture")
	capture.calendarSpeed(nil, value)
	return run(api.cmd.makeGameSetCalendarSpeedCmd(value))
end

-- A town building's Historic Preservation (action::Preservation), as its
-- window sets it (gui/entity_window/town_building/town_building.tl): the
-- town building at that place in the construction's list. Town buildings
-- are the town's: any company may, as in single player.
function HANDLERS.Preserve(p)
	local con, c = constructionAt(p.building)
	local list = c and c.townBuildings
	local building = list and list[p.index + 1]
	if type(building) ~= "number" then
		error("no town building " .. tostring(p.index) .. " in the " .. tostring(p.building.file), 0)
	end
	log((p.preserved and "preserving " or "no longer preserving ") .. tostring(building) .. " of "
		.. tostring(con) .. " " .. tostring(p.building.file))
	return run(api.cmd.makeTownBuildingSetBlockedDevelopmentCmd(building, p.preserved == true))
end

-- A level crossing's type, from its window (tpf3mp/crossings.lua): the one
-- crossing whose nodes are exactly where the action names them, if the
-- acting company may change it (its own or no one's: the crossing and every
-- road and track of it, an owner that does not read refused), takes the type
-- as the window gives it: an empty build the hook fills with the crossing
-- and type (crates/tpf3mp-hook/src/crossings.rs). The type is read back:
-- unchanged, the game did not take it (the same in every game); anything
-- else, or no crossing left, stops the replay, as the worlds may differ.
function HANDLERS.SetCrossingType(change)
	local crossings = module("crossings")
	local entity, c = crossings.resolve(change.nodes)
	local index = crossings.typeIndex(change.crossing_type)
	local PLAYER_OWNED = api.type.ComponentType.PLAYER_OWNED
	local function may(e, what)
		local read = pcall(function()
			local owned = api.engine.getComponent(e, PLAYER_OWNED)
			return owned and owned.player
		end)
		if not read then error("the owner of the " .. what .. " does not read", 0) end
		mine(e, what)
	end
	may(entity, "level crossing")
	local edges = c.edges
	for i = 1, (edges and #edges or 0) do may(edges[i], "level crossing's road or track") end
	local before = c.typeIndex
	if before == index then
		log("crossing " .. tostring(entity) .. " is of type " .. tostring(change.crossing_type) .. " already")
		return true
	end
	local _, nodes = crossings.places(c)
	-- The game's script API reaches only node2rcType, which types the
	-- crossings a proposal lays anew; the window retypes one in place
	-- through a map of its own (crossing -> type), which the hook fills in
	-- an empty build here, as the window's own (docs/HOOKS.md, "Level
	-- crossing types").
	if type(apply.crossing) ~= "function" then error("this hook cannot set a crossing's type", 0) end
	local context = api.type.Context.new()
	context.player = company()
	-- What the change cost the company, for the log: the same in every game.
	local function balance()
		local ok, account = pcall(api.engine.getComponent, context.player, api.type.ComponentType.ACCOUNT)
		local value = ok and account and account.balance
		return type(value) == "number" and value or nil
	end
	local paidFrom = balance()
	local command = api.cmd.makeWorldBuildProposalCmd(api.type.Proposal.new(), context, true, true)
	-- Armed for this one build only: disarmed whatever becomes of it, so no
	-- later build of the room's is filled with it.
	local armed, why = apply.crossing(entity, index)
	if armed ~= true then
		apply.crossing(nil)
		error("the hook would not take the crossing: " .. tostring(why), 0)
	end
	local sent, err = pcall(run, command)
	local filled = apply.crossing(nil)
	if not sent then error(err, 0) end
	if filled ~= true then error("the hook filled no build with the crossing", 0) end
	-- What the game made of it: the crossing on the same nodes, which the
	-- game makes anew (it removes the old one and adds one of the type).
	local okAfter, after = pcall(api.engine.system.railRoadCrossingSystem.getRailroadCrossingForNode, nodes[1])
	local now = okAfter and crossings.component(after)
	if not now then
		error({ replayFatal = "level crossing " .. tostring(entity) .. " is gone after its type change" }, 0)
	end
	if now.typeIndex == before then
		error("the game did not change the crossing's type", 0)
	end
	if now.typeIndex ~= index then
		error({ replayFatal = "level crossing " .. tostring(entity) .. " took type " .. tostring(now.typeIndex)
			.. ", not " .. tostring(change.crossing_type) }, 0)
	end
	local paidTo = balance()
	local paid = (paidFrom and paidTo) and string.format("%.0f", paidFrom - paidTo) or "?"
	log("crossing " .. tostring(entity) .. " (now " .. tostring(after) .. ") set to type "
		.. tostring(change.crossing_type) .. ", the company paid " .. paid)
	return true
end

-- The room's companies (tpf3mp/companies.lua): the acting player founds,
-- joins, renames, recolours or dissolves one, and its head locks it, sends a
-- player out or shares its stations, in `ctx.roster`; `ctx.seal` is the
-- room's seal of a password sent with it.
function HANDLERS.CompanyOp(op, ctx)
	if not (ctx and ctx.roster and ctx.player) then return false, "no roster to change" end
	local ok, why = companiesModule.run(ctx.roster, ctx.player, op, send, api, ctx.seal)
	if not ok then return false, why end
	return true
end

-- Taking a company rank (tpf3mp/progression.lua). With one company in the
-- room, the company growth script's own event, as the company window sends
-- it: the game keeps that company's rank, and checks the rank is reached.
-- With more, the acting company's rank in the mod's state, when it reached
-- it; the save's own player also takes it in the game's own state, so its
-- rank stays when the room is one company again.
function HANDLERS.ApplyRank(r, ctx)
	local level = tonumber(r.level)
	if level == nil then return false, "a rank is a number" end
	local roster = ctx and ctx.roster
	local event = function()
		return api.cmd.makeScriptingSendEventCmd("", "Companies", "applyLevel", { level = level })
	end
	if progressionModule.multi(roster) then
		local ok, why = progressionModule.take(ctx.progression, roster, company(), level)
		if not ok then return false, why end
		if company() == api.engine.util.getPlayer() then send(event()) end
		return true
	end
	if company() ~= api.engine.util.getPlayer() then
		return false, "only the room's first company has the game's own rank"
	end
	-- The game ignores a rank not reached; this says why, where it can read it.
	local game = progressionModule.game(api)
	local read, own = pcall(function() return game and game.own(company()) end)
	if read and type(own) == "table" and type(own.level) == "number" and type(own.potentialLevel) == "number" then
		if level <= own.level then return false, "the company has rank " .. own.level .. " already" end
		if level > own.potentialLevel then
			return false, "the company has reached rank " .. own.potentialLevel .. ", not " .. level
		end
	end
	return run(event())
end

local COMPANY_OPERATIONS = {
	"Create", "Join", "Rename", "Recolor", "Delete", "Lock", "Unlock", "Dismiss",
	"ShareStations", "StationAccess",
}

local function diagnosticCompanyOperation(op)
	for _, name in ipairs(COMPANY_OPERATIONS) do
		local _, status = diagnosticField(op, name)
		if status == "value" then return name end
	end
	return "other"
end

local function diagnosticOrderedModels(buy)
	local consist, status = diagnosticField(buy, "consist")
	if status ~= "value" then return status end
	local length = diagnosticSequenceLength(consist)
	if length == nil then return "unavailable" end
	if length == 0 then return "none" end
	local names = {}
	local count = math.min(length, 8)
	for i = 1, count do
		local part, partStatus = diagnosticField(consist, i)
		local model, modelStatus = diagnosticField(part, "model")
		if partStatus ~= "value" then
			names[#names + 1] = partStatus
		elseif modelStatus == "value" and type(model) == "string" and model ~= "" then
			names[#names + 1] = diagnosticToken(model)
		else
			names[#names + 1] = modelStatus
		end
	end
	if length > count then names[#names + 1] = "+" .. tostring(length - count) end
	return table.concat(names, ",")
end

-- Called by the simulation after the registry has seen the command result.
-- Only the ordered resource names and native/canonical result IDs are kept;
-- the action, consist configuration, and command result are never serialized.
function apply.vehicleBuyDiagnostic(buy, applied, entity, canonical, canonicalStatus, actionIndex)
	local outcome = applied == true and "applied" or "refused"
	local native = entity == nil and "none" or diagnosticEntity(entity)
	local canonicalText = "unavailable"
	if canonicalStatus == "none" then
		canonicalText = "none"
	elseif canonicalStatus == "value" then
		canonicalText = diagnosticEntity(canonical)
	end
	return "buy-vehicle action_index=" .. diagnosticEntity(actionIndex)
		.. " outcome=" .. outcome .. " models=" .. diagnosticOrderedModels(buy)
		.. " native=" .. native .. " canonical=" .. canonicalText
end

local function diagnosticOutcome(ok, applied)
	if not ok and type(applied) == "table" and applied.replayFatal then return "uncertain" end
	if ok and applied == true then return "applied" end
	return "refused"
end

local function diagnosticLog(line)
	pcall(log, line)
end

-- Runs one action. `ctx` is { registry = } (tpf3mp/registry.lua), for the
-- actions that name vehicles, lines and station groups; with companies, also
-- `roster`, `player` (who sent it) and `company` (their company's player
-- entity), which the action is booked to, and `progression`, the companies'
-- ranks (tpf3mp/progression.lua). Returns true, nil
-- and the entity it made or changed (for the kinds in CREATES and KEEPS,
-- where the game said), or false and why not; never raises. A fourth true
-- marks a post-command read failure: the caller must stop ordered replay
-- before persisting any inferred registry mapping.
function apply.run(action, ctx)
	if type(action) ~= "table" then return false, "an action is a table" end
	local allowed, why = acceptance.check(action)
	if not allowed then return false, why end
	local kind, body = next(action)
	if kind == nil or next(action, kind) ~= nil then
		return false, "an action is a table of one entry"
	end
	local handler = HANDLERS[kind]
	if handler == nil then
		return false, "this version of the mod does not apply " .. tostring(kind) .. " yet"
	end
	local actorBefore, localBefore = "unavailable", "unavailable"
	if kind == "CompanyOp" then
		local readActor, valueActor = pcall(diagnosticActor, ctx and ctx.roster, ctx and ctx.player)
		local readLocal, valueLocal = pcall(diagnosticLocal, ctx and ctx.roster)
		if readActor then actorBefore = valueActor end
		if readLocal then localBefore = valueLocal end
	end
	acting = ctx
	costStart(kind)
	local ok, applied, detail = pcall(handler, body, ctx)
	local cost = costEnd()
	acting = nil
	if cost and cost.ms >= apply.SLOW_MS then
		local parts = apply.costText(cost)
		log(string.format("applying %s took %.1f ms", kind, cost.ms) .. (parts ~= "" and (": " .. parts) or ""))
	end
	local outcome = diagnosticOutcome(ok, applied)
	if kind == "CompanyOp" then
		local actorAfter, localAfter = "unavailable", "unavailable"
		local readActor, valueActor = pcall(diagnosticActor, ctx and ctx.roster, ctx and ctx.player)
		local readLocal, valueLocal = pcall(diagnosticLocal, ctx and ctx.roster)
		if readActor then actorAfter = valueActor end
		if readLocal then localAfter = valueLocal end
		local readOp, operation = pcall(diagnosticCompanyOperation, body)
		if not readOp then operation = "other" end
		diagnosticLog("company-event action_index=" .. diagnosticActionIndex(ctx) .. " op=" .. operation
			.. " actor_before=" .. actorBefore .. " actor_after=" .. actorAfter
			.. " local_before=" .. localBefore .. " local_after=" .. localAfter
			.. " outcome=" .. outcome)
	elseif kind == "AssignLine" or kind == "Bulldoze" then
		diagnosticLog("action-event action_index=" .. diagnosticActionIndex(ctx) .. " kind=" .. kind
			.. " outcome=" .. outcome)
	end
	if not ok and type(applied) == "table" and applied.replayFatal then
		return false, applied.replayFatal, nil, true
	end
	if not ok then return false, tostring(applied) end
	if applied == true then return true, nil, detail end
	return false, detail
end

-- The actions whose proposal apply.proposalOf makes: the builds a player's
-- tool previews (tpf3mp/previews.lua).
apply.PREVIEWS = { BuildConstruction = true, BuildRoad = true, BuildTrack = true, PlaceStop = true }

-- The proposal `action` would build in this game, and the context it would
-- be built with, as apply.run would make them for `ctx`, without sending
-- anything, building anything or writing the log: for showing another
-- player's build preview (docs/HOOKS.md, "Build previews"). Or nil and why:
-- an action that is not a build, or one this game cannot make (a street
-- type it lacks, an edge it does not have).
function apply.proposalOf(action, ctx)
	if type(action) ~= "table" then return nil, "an action is a table" end
	local kind, body = next(action)
	if kind == nil or next(action, kind) ~= nil then return nil, "an action is a table of one entry" end
	if not apply.PREVIEWS[kind] then return nil, "no preview of " .. tostring(kind) end
	local allowed, why = acceptance.check(action)
	if not allowed then return nil, why end
	dry, acting = true, ctx
	costStart(kind)
	local ok, stopped = pcall(HANDLERS[kind], body, ctx)
	costEnd()
	dry, acting = false, nil
	if not ok and type(stopped) == "table" and stopped.proposal ~= nil then
		return stopped.proposal, stopped.context
	end
	if ok then return nil, "the build made no proposal" end
	if type(stopped) == "table" then return nil, tostring(stopped.dry) end
	return nil, tostring(stopped)
end

-- The actions this version applies, for tests and the log.
function apply.kinds()
	local kinds = {}
	for kind in pairs(HANDLERS) do kinds[#kinds + 1] = kind end
	table.sort(kinds)
	return kinds
end

return apply
