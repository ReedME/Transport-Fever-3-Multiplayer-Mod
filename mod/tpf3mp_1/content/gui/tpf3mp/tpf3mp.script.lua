-- TPF3-MP in the game's GUI state: the plugin gui/tpf3mp/tpf3mp.res.lua
-- names. On the first step of a game it loads the mod's modules and links
-- to the hook (tpf3mp/bridge.lua). Without a hook, which is every game Steam
-- started, it logs one line and does nothing more. The room's actions are
-- applied by the mod's game script (tpf3mp_sim/), not here.
--
-- Every frame it does what the hook asks (docs/HOOKS.md, "The room's
-- world"): save the world under a name when the room orders a save, or load
-- the room's world from the game's save folder. It tells the hook each time
-- a world's GUI starts, which is how the hook sees a load finish.
--
-- Once linked, it puts the guard in front of the GUI's commands
-- (tpf3mp/guard.lua; docs/HOOKS.md, "The player's commands"): in the room's
-- game a command the room carries goes to the room, and its window hears
-- what became of it once this game has applied it; a command the room
-- cannot carry yet is refused, and the game bar says so for a few seconds.
-- The guard names vehicles, lines, station groups and towns by the canonical
-- ids the mod's game script keeps in its state (tpf3mp/registry.lua).
--
-- It follows what mods made for Transport Fever 3 build 40391 rely on
-- (investigation/TF3_MODS_2026-09-27.md): a .script.lua defines data();
-- ug_require loads the game's files ("::/...") and a mod's own
-- ("tpf3mp_1::/..."); a GameBarInfoDisplayExtension plugin with
-- react.onStep runs code every frame in a game; debugPrint writes to the
-- game's log. Each step logs "[tpf3mp]" lines, so the log shows how far a
-- game got on release day.
--
-- In the room's game the game bar also shows the room in one line (its
-- name, who is there, its speed, whether the worlds match), a button that
-- opens the Multiplayer window: the room, its players, its speed, whether
-- this world matches the room's, and the room's chat, which the player can
-- write to (docs/PLAN.md: the in-game Multiplayer panel for a game the
-- launcher started; the lobby stays in the launcher). The game's window
-- container shows the window, and the game's area for mods' buttons has a
-- second button for it (a second plugin, tpf3mp_button.res.lua). What
-- they show is kept in package.loaded["tpf3mp.ui"], which the game bar
-- plugin fills every frame from the hook (tpf3mp/bridge.lua: status, chat,
-- say).
function data()
	local MOD = "tpf3mp_1"
	-- Every module, in an order where each needs only those before it.
	local MODULES = { "acceptance", "banners", "geom", "roads", "engine", "registry", "companies", "progression", "follow", "capture",
		"bridge", "guard", "hudguard", "worldload", "junctions", "apply", "previews" }
	-- Frames a refusal's notice stays in the game bar.
	local NOTICE_FRAMES = 360

	local function say(line)
		pcall(debugPrint, "[tpf3mp] " .. line)
	end

	-- The link to the hook, once a world's GUI has found it.
	local link = nil

	-- What the Multiplayer window and the game bar show, shared by both
	-- plugins: the room (link:status()), the chat so far, whether the window
	-- is open, and a count that goes up whenever any of it changes; and the
	-- link, which the game bar plugin makes: the game may run this file
	-- once for each plugin, each with its own locals.
	local function ui()
		local shared = package.loaded["tpf3mp.ui"]
		if type(shared) ~= "table" then
			shared = { open = false, status = nil, lines = {}, unread = 0, version = 0 }
			package.loaded["tpf3mp.ui"] = shared
		end
		return shared
	end

	-- Callbacks of refused commands, for the next frame, as the game would
	-- call them.
	local pending = {}
	-- The notice of the last refusal, until the plugin shows it.
	local notice = nil
	-- The last refusal the game script's GUI half told under
	-- "tpf3mp.notice" (a native click it stopped: the level crossing
	-- window's), or false before the first read, which only notes it.
	local toldNotice = false
	-- Refusals so far, by kind, and the last reason logged of each, for the
	-- hook's log.
	local refusals, reasons = {}, {}

	local function refused(kind, why, from)
		local name = kind or "command no factory made"
		local count = (refusals[name] or 0) + 1
		refusals[name] = count
		local changed = why ~= nil and why ~= reasons[name]
		if changed then reasons[name] = why end
		if count == 1 or count % 100 == 0 or changed then
			link:log("refused the player's " .. name .. " in the room's game ("
				.. count .. " so far)" .. (why and (": " .. tostring(why)) or "")
				.. (from and (", from the mod " .. tostring(from)) or ""))
		end
		notice = require("tpf3mp.guard").notice(kind)
	end

	local function runPending()
		if #pending == 0 then return end
		local due = pending
		pending = {}
		for _, fn in ipairs(due) do
			local ok, err = pcall(fn)
			if not ok then say("a refused command's callback failed: " .. tostring(err)) end
		end
	end

	-- The mod's game script's state as the game keeps it
	-- (tpf3mp/companies.lua), which holds its registry (tpf3mp/registry.lua)
	-- and its companies. Read once the modules are loaded.
	local function scriptState()
		return require("tpf3mp.companies").scriptState(api)
	end
	local function registryNow()
		local state = scriptState()
		return state and state.registry
	end

	-- What the guard names things by (tpf3mp/capture.lua).
	local function idOf(kind)
		return function(entity)
			return require("tpf3mp.registry").id(registryNow(), kind, entity)
		end
	end
	local context = {
		vehicle = idOf("vehicles"),
		line = idOf("lines"),
		group = idOf("groups"),
		town = idOf("towns"),
		-- An industry by its construction (capture.industryConstruction).
		industry = idOf("industries"),
		-- The room's company whose player entity `entity` is, by its id: the
		-- game's company window renames the player's company so
		-- (game_mechanics/company/company.tl, its editable title).
		company = function(entity)
			local roster = ui().companies
			for _, c in ipairs(roster and roster.list or {}) do
				if c.entity == entity then return c.id end
			end
			return nil
		end,
		player = function()
			local ok, player = pcall(function() return api.engine.util.getPlayer() end)
			if ok and type(player) == "number" then return player end
			return nil
		end,
		-- A depot by its construction (capture.depotRef): a street's, a
		-- harbour's or an airport's.
		depot = function(depot) return require("tpf3mp.capture").depotRef(api, depot) end,
		-- The company or player owning an entity (PLAYER_OWNED), for the log.
		owner = function(entity)
			local ok, owned = pcall(function() return api.engine.getComponent(entity, api.type.ComponentType.PLAYER_OWNED) end)
			return ok and owned and owned.player or nil
		end,
		-- What the capture tells the log.
		say = function(text) if link then link:log(text) end end,
		model = function(id)
			local ok, name = pcall(function() return api.res.modelRep.getName(id) end)
			if ok and type(name) == "string" and name ~= "" then return name end
			return nil
		end,
		subsidy = function(uid)
			local companies = require("tpf3mp.companies")
			local where, s = companies.findSubsidy(companies.subsidyState(api), uid)
			if where == "offered" and type(s.id) == "string" then return s.id end
			return nil
		end,
		-- A vehicle's parts as its TRANSPORT_VEHICLE component has them.
		parts = function(vehicle)
			local ok, parts = pcall(function()
				local tv = api.engine.getComponent(vehicle, api.type.ComponentType.TRANSPORT_VEHICLE)
				local list = tv.transportVehicleConfig.vehicles
				local out = {}
				for i = 1, #list do
					out[i] = { model = list[i].part.modelId, purchased = list[i].purchaseTime }
				end
				return out
			end)
			if ok then return parts end
			return nil
		end,
	}

	-- The GUI state's api.cmd, which the guard is on.
	local guardedCmd = nil
	local answers = {}

	-- Whether the GUI's world has `entity` yet: what the room's action made
	-- in the simulation reaches it a moment later. With `kind`, also whether
	-- the registry names it yet (tpf3mp/guard.lua, NAMED): the game script's
	-- state, which binds its id, reaches the GUI later still.
	local function sees(entity, kind)
		local ok, there = pcall(function() return api.engine.entityExists(entity) end)
		if ok and there ~= true then return false end
		if kind == nil then return true end
		local okId, id = pcall(function() return idOf(kind)(entity) end)
		return okId and id ~= nil
	end

	-- Seconds by the wall clock, for how long an answer waits on what its
	-- command made (tpf3mp/guard.lua, HOLD_SECONDS); nil without one.
	local function clock()
		local ok, t = pcall(function() return os.time() end)
		if ok and type(t) == "number" then return t end
		return nil
	end

	-- Puts the guard in front of the GUI's commands.
	local function guardCommands()
		local ok, cmd = pcall(function() return api.cmd end)
		guardedCmd = ok and cmd or nil
		local wrapped, why = require("tpf3mp.guard").install(guardedCmd, {
			inRoom = function() return link:room() end,
			command = function(action)
				local ok, ticket = link:command(action)
				local subsidy = ok and type(action) == "table" and action.Subsidy
				if subsidy and ticket ~= nil then
					answers[ticket] = subsidy.Accept and "Taking the subsidy" or "Declining the subsidy"
				end
				return ok, ticket
			end,
			refused = refused,
			later = function(fn) pending[#pending + 1] = fn end,
			context = context,
			personal = function(mod) return link:personal()[mod] == true end,
			shared = function() return link:shared() end,
		})
		if wrapped then
			link:log("the guard is on " .. wrapped .. " command factories")
		else
			link:log("the guard is not on: " .. tostring(why)
				.. "; the player's commands are not checked")
		end
	end

	-- The modules name each other `require "tpf3mp.<name>"`, as TPF2's
	-- did. The game's GUI state has `require` and package.loaded but no
	-- package.preload (build 40408's dump,
	-- investigation/dayone-2026-09-29/probe/script_api_dump_gui.txt), so
	-- each module is loaded here through ug_require, in order, into
	-- package.loaded, where the others' `require` finds it. Only names
	-- under "tpf3mp." are added, so nothing else in the state changes.
	local function installModules()
		if type(package) ~= "table" or type(package.loaded) ~= "table" then
			return nil, "this Lua state has no package.loaded"
		end
		for _, name in ipairs(MODULES) do
			local key = "tpf3mp." .. name
			if package.loaded[key] == nil then
				local path = MOD .. "::/scripts/tpf3mp/" .. name .. ".lua"
				local ok, module = pcall(ug_require, path)
				if not ok or module == nil then
					return nil, key .. " did not load: " .. tostring(module)
				end
				package.loaded[key] = module
			end
		end
		return true
	end

	-- The player entity of the company this player plays for, in the room's
	-- game (tpf3mp/companies.lua), or nil: outside the room, before the
	-- roster is read, and for the room's first company, which is the save's
	-- own player anyway.
	local function myCompany()
		local shared = ui()
		local status = shared.status
		return require("tpf3mp.follow").companyOf(shared.companies, status and status.me_id)
	end

	-- The GUI's "my company" in this Lua state (tpf3mp/follow.lua).
	local function followMyCompany()
		local follow = require("tpf3mp.follow")
		local ok, why = follow.install(api, myCompany, function(line) link:log(line .. " (the Multiplayer plugin's state)") end)
		pcall(follow.watchLines, api, ug_require, link, "the Multiplayer plugin's state")
		link:log(ok and "the GUI's company follows the player's"
			or ("the GUI's company cannot follow the player's: " .. tostring(why)))
	end

	-- The company window's ranks and the permits they give, with more than
	-- one company in the room: each company's own, as the room's rule keeps
	-- them (tpf3mp/progression.lua), read from the mod's game script's
	-- state. With one company the game's own.
	-- The game's permit counts, as the construction menu and tool read them,
	-- count the player's company's own constructions while the room has
	-- more than one company (tpf3mp/companies.lua, followPermits): each its
	-- own headquarters.
	local function countOwnPermits()
		local ok, why = require("tpf3mp.companies").followPermits(api, ug_require, function()
			local roster = ui().companies
			return roster ~= nil and #(roster.list or {}) > 1
		end)
		link:log(ok and ("the game's permits count each company's own constructions (" .. ok .. " company_util table(s))")
			or ("the game's permits count the whole world's constructions: " .. tostring(why)))
	end

	local function showRanks()
		local ok, why = require("tpf3mp.progression").follow(scriptState)
		link:log(ok and "the company window shows each company's own rank"
			or ("the company window shows the game's own rank only: " .. tostring(why)))
	end

	-- Install in this GUI state too; the HUD and game-script GUI install
	-- the same helper in their own states.
	local function offerOpenStations()
		local changed = require("tpf3mp.companies").followStations(api, ug_require, function()
			local shared = ui()
			if link and link:room() and shared.status then
				return shared.companies, shared.status.me_id
			end
		end)
		link:log(changed > 0 and ("the line manager offers other companies' open stations ("
			.. changed .. " entity_util table(s))")
			or "the line manager offers the player's own stations only: no entity_util.isOwnedByPlayerOrNotOwned")
	end

	local function start()
		local ok, why = installModules()
		if not ok then
			say("not started: " .. why)
			return
		end
		say("modules loaded")

		local bridge = require "tpf3mp.bridge"
		local found, reason = bridge.attach(bridge.find())
		if not found then
			say(reason .. "; this is the plain game")
			return
		end
		link = found
		ui().link = link
		link:world()
		link:log("the GUI is linked")
		say("linked to the hook")
		guardCommands()
		followMyCompany()
		showRanks()
		countOwnPermits()
		offerOpenStations()
		-- The stop the construction menu gives the stop tool, wherever the
		-- menu runs (gui/tpf3mp/gui_state.script.lua watches the other state).
		pcall(function()
			require("tpf3mp.capture").watchStopTool(ug_require "::/gui/construction/construction_react_util.tl", link)
		end)
	end

	-- The room's world being loaded (tpf3mp/worldload.lua), a frame at a
	-- time while the game reads the save's mods.
	local loading

	-- Does what the hook asks: saving the world under the name it gives, or
	-- loading the room's world from the game's save folder, with the room's
	-- mods (docs/MODS.md).
	local function serve()
		if not link then return end
		local request = link:poll()
		if request and request.replay then
			local ok, why = require("tpf3mp.guard").wakeReplay(guardedCmd, request.replay)
			if not ok then link:replayed(request.replay, false, why) end
		elseif request and request.save then
			local name = request.save
			local ok, err = pcall(app.saveGame, name, function()
				link:saved(name, true)
			end, false, true)
			if not ok then link:saved(name, false, tostring(err)) end
		elseif request and request.load then
			loading = require("tpf3mp.worldload").new(request.load)
		end
		if loading then
			local done, why = require("tpf3mp.worldload").step(loading, app, api, link)
			if done == "busy" then return end
			loading = nil
			if done == "started" then
				link:log("loading the room's world")
			else
				link:log("loading the room's world failed: " .. tostring(why))
			end
		end
	end

	local readCompanies
	local react = ug_require "::/gui/main/react.lua"
	local builtin = ug_require "::/gui/main/builtin.lua"
	local game_bar_widgets = ug_require "::/gui/game_bar/game_bar_widgets.tl"
	local main_mod_button_area = ug_require "::/gui/main/main_mod_button_area.tl"
	local game_react_globals = ug_require "::/gui/main/game_react_globals.tl"

	-- Chat lines kept, newest last, and how many of them the window shows.
	local CHAT_LINES = 50
	local CHAT_SHOWN = 12
	-- Frames between two readings of the room.
	local STATUS_FRAMES = 15

	-- The room's speed as the speed row says it.
	local function speedText(speed)
		if speed == nil then return "" end
		if speed == 0 then return "paused" end
		return string.format("%gx", speed / 100)
	end

	-- The room in one line, for the game bar.
	local function summary(status)
		local here, all = 0, 0
		for _, p in ipairs(status.players or {}) do
			all = all + 1
			if p.connected then here = here + 1 end
		end
		local parts = { "Multiplayer: " .. tostring(status.room), here .. "/" .. all .. " playing" }
		if status.speed then parts[#parts + 1] = speedText(status.speed) end
		if status.diverged then parts[#parts + 1] = "resyncing" end
		return table.concat(parts, " · ")
	end

	-- The room's companies as the game script keeps them (tpf3mp/companies.lua),
	-- each with its money now, and a text that changes when anything shown
	-- does. Nil before the room's first company exists.
	readCompanies = function()
		local state = scriptState()
		local roster = state and state.companies
		if type(roster) ~= "table" or type(roster.list) ~= "table" then return nil, "" end
		local out, sign = { list = {}, members = roster.members or {}, loans = roster.loans or {} }, {}
		out.passwordNotice = require("tpf3mp.companies").passwordNotice(roster)
		if out.passwordNotice then sign[#sign + 1] = out.passwordNotice end
		-- A founded company reads its own persisted offers. Company zero keeps
		-- the native finance window and its loan script state.
		pcall(function()
			local status = ui().status
			local companies = require("tpf3mp.companies")
			local follow = require("tpf3mp.follow")
			local entity = follow.companyOf(roster, status and status.me_id)
			local own = entity and companies.byEntity(roster, entity)
			if not own or own.id == 0 then return end
			local e = api.engine.system.gameScriptSystem.getEntityForGameScript("::/game_mechanics/finance/loan.gs")
			local c = type(e) == "number" and e >= 0 and api.engine.getComponent(e, api.type.ComponentType.GAME_SCRIPT)
			local real = c and c.state
			local month = api.util.getDefaultMonthDuration()
			local loans = companies.loanTable(roster, own.id, real, month)
			if type(loans.availableLoans) == "table" then out.offers = loans.availableLoans end
		end)
		for _, offer in ipairs(out.offers or {}) do
			sign[#sign + 1] = table.concat({ tostring(offer.type), tostring(offer.amount),
				tostring(offer.duration), tostring(offer.percentage), tostring(offer.cooldownUntil) }, ":")
		end
		for _, loan in ipairs(out.loans) do sign[#sign + 1] = loan.id .. ":" .. loan.remaining end
		for _, c in ipairs(roster.list) do
			if not c.gone then
				local balance, owed, name
				pcall(function()
					local account = api.engine.getComponent(c.entity, api.type.ComponentType.ACCOUNT)
					balance = account and account.balance
					owed = account and account.loan
				end)
				-- The money the game's own windows show (the finance window,
				-- the game bar: api.engine.util.finance.getPlayersBalance),
				-- where the game answers. The room's first company's card
				-- showed $0 in a real game (build 40408, 2026-09-30) while
				-- the game bar showed its money; INFERRED that its ACCOUNT
				-- component does not hold what the game shows.
				pcall(function()
					local shown = api.engine.util.finance.getPlayersBalance(c.entity)
					if type(shown) == "number" then balance = shown end
				end)
				-- The name the game shows (the player entity's NAME, which a
				-- rename sets), else the roster's.
				pcall(function()
					local n = api.engine.getComponent(c.entity, api.type.ComponentType.NAME)
					if n and type(n.name) == "string" and n.name ~= "" then name = n.name end
				end)
				-- Whether it has a password, not the password's seal: the
				-- window has no use for it.
				local locked = type(c.lock) == "table"
				out.list[#out.list + 1] = { id = c.id, entity = c.entity, name = name or c.name, color = c.color,
					balance = balance, owed = owed, founder = c.founder, locked = locked, closed = c.closed == true,
					access = c.access }
				for _, a in ipairs(c.access or {}) do sign[#sign + 1] = c.id .. ">" .. tostring(a.company) .. "=" .. tostring(a.open) end
				local color = type(c.color) == "table" and c.color or {}
				sign[#sign + 1] = table.concat({ c.id, name or c.name, tostring(balance), tostring(owed),
					tostring(color[1]), tostring(color[2]), tostring(color[3]), tostring(locked),
					tostring(c.closed == true), tostring(c.founder) }, ":")
			end
		end
		for _, m in ipairs(out.members) do sign[#sign + 1] = tostring(m.player) .. "=" .. tostring(m.company) end
		return out, table.concat(sign, "|")
	end

	-- Reads the room and its chat from the hook into ui(): the room every
	-- STATUS_FRAMES frames, the chat every frame.
	local statusFrames = 0
	local function follow()
		if not link then return end
		local shared = ui()
		local changed = false
		statusFrames = statusFrames - 1
		if statusFrames <= 0 then
			statusFrames = STATUS_FRAMES
			local status = link:status()
			local before = shared.status and summary(shared.status) .. tostring(#(shared.status.players or {}))
			local after = status and summary(status) .. tostring(#(status.players or {}))
			shared.status = status
			if before ~= after then changed = true end
			local companies, sign = readCompanies()
			shared.companies = companies
			if sign ~= shared.companiesSign then
				shared.companiesSign = sign
				changed = true
			end
			-- The company the native tools act as, for the hook
			-- (tpf3mp/follow.lua, noteCompany).
			shared.companyNoted = require("tpf3mp.follow").noteCompany(link, myCompany(), shared.companyNoted)
			shared.companiesNoted = require("tpf3mp.follow").noteCompanies(link, shared.companies, shared.companiesNoted)
		end
		-- A new world's GUI gets the chat so far again, as old lines: they
		-- fill the window without counting as new.
		for _, line in ipairs(link:chat()) do
			shared.lines[#shared.lines + 1] = tostring(line.from) .. ": " .. tostring(line.text)
			if #shared.lines > CHAT_LINES then table.remove(shared.lines, 1) end
			if not shared.open and not line.old then shared.unread = shared.unread + 1 end
			changed = true
		end
		if changed then shared.version = shared.version + 1 end
	end

	-- What the Multiplayer window shows: the room, its speed, whether this
	-- world matches the room's, its players, and the chat with a field to
	-- write to it.
	-- Money as the game bar writes it, near enough.
	local function money(balance)
		if type(balance) ~= "number" then return "" end
		local sign, whole = balance < 0 and "-" or "", tostring(math.floor(math.abs(balance) + 0.5))
		whole = whole:reverse():gsub("(%d%d%d)", "%1,"):reverse():gsub("^,", "")
		return sign .. "$" .. whole
	end

	local function vec3(color)
		if type(color) ~= "table" then return nil end
		return api.type.Vec3f.new(color[1] or 0, color[2] or 0, color[3] or 0)
	end

	-- A company operation for the room, from the window. What became of it
	-- comes back with the player's other actions (follow the ticket). A
	-- password, for joining or locking a company, goes to the room beside it
	-- (tpf3mp/bridge.lua); `doing` never names it.
	local function companyOp(shared, op, doing, action, password)
		local l = shared.link
		if not l then return end
		local ok, ticket = l:command(action or { CompanyOp = op }, password)
		if ok then
			shared.asked = shared.asked or {}
			shared.asked[ticket] = doing
			shared.companyNote = doing .. "..."
		else
			shared.companyNote = "Not sent: " .. tostring(ticket)
		end
		shared.version = shared.version + 1
	end

	-- The colours a company can wear: the companies' own first (a vehicle in
	-- one has its marker on the map in it too), then the game's line colours
	-- and greys, as its vehicle and line windows offer them
	-- (gui/line_vehicle_mgmt/line_react_util.tl). The chooser also takes a
	-- colour of the player's own (INFERRED from its style sheet's
	-- custom-color-button, gui/main/builtin.css.lua).
	local palette = nil
	local function companyPalette()
		if palette then return palette end
		palette = {}
		for _, color in ipairs(require("tpf3mp.companies").PALETTE) do palette[#palette + 1] = vec3(color) end
		pcall(function()
			local rep = api.gui.genericRep
			local color_util = ug_require("/gui/main/color_util.tl")
			for _, file in ipairs({ "::/gui/line_vehicle_mgmt/line_colors.gres", "::/gui/main/grayscale.gres" }) do
				for _, color in ipairs(color_util.toArray3(rep.get(rep.find(file)).data)) do
					palette[#palette + 1] = color
				end
			end
		end)
		return palette
	end

	-- Native fixed-width panels keep a long name, eight companies or a busy
	-- chat from stretching the window beyond the screen. -1 means automatic
	-- height in TF3's StyleSheet; zero would hide the content.
	local WINDOW_WIDTH, PLAYERS_WIDTH, COMPANIES_WIDTH = 800, 192, 548
	local BODY_HEIGHT = 310
	local COPIED_FRAMES = 120
	local function sheet(width, height, padding, background)
		local s = api.gui.StyleSheet.new()
		s.size = api.type.Vec2f.new(width or -1, height or -1)
		if padding then s.padding = api.type.Vec4f.new(padding, padding, padding, padding) end
		if background then s.backgroundColor = api.type.Vec4f.new(0.07, 0.10, 0.12, 0.90) end
		return s
	end
	local function gap(size)
		return builtin.Component{ meta = { styleSheet = sheet(size, size) },
			mouseTransparent = true, layout = builtin.BoxLayout{ children = {} } }
	end
	local function box(children, horizontal, width, padding, background)
		return builtin.Component{
			meta = { styleSheet = sheet(width, nil, padding, background) },
			mouseTransparent = true,
			layout = builtin.BoxLayout{
				orientation = horizontal and builtin.type.Orientation.Horizontal or builtin.type.Orientation.Vertical,
				children = children,
			},
		}
	end
	-- Break on spaces when possible and on UTF-8 character boundaries for
	-- unbroken names. The complete text stays visible, including chat.
	local function wrapped(text, limit)
		local lines, chars, space = {}, {}, nil
		local function flush(count)
			lines[#lines + 1] = table.concat(chars, "", 1, count)
			local left = {}
			for i = count + 1, #chars do left[#left + 1] = chars[i] end
			if left[1] == " " then table.remove(left, 1) end
			chars, space = left, nil
			for i, c in ipairs(chars) do if c == " " then space = i end end
		end
		for c in tostring(text):gmatch("[%z\1-\127\194-\244][\128-\191]*") do
			if c == "\n" then
				flush(#chars)
			else
				chars[#chars + 1] = c
				if c == " " then space = #chars end
				if #chars > limit then flush(space and space > 1 and space - 1 or limit) end
			end
		end
		if #chars > 0 then lines[#lines + 1] = table.concat(chars) end
		return table.concat(lines, "\n")
	end
	local function label(text, class, width, limit)
		return builtin.TextView{
			meta = { class = class or "font-scale-body", styleSheet = sheet(width), tooltip = tostring(text) },
			text = limit and wrapped(text, limit) or tostring(text),
		}
	end
	local function scroll(children, width, height)
		return builtin.ScrollArea{
			meta = { styleSheet = sheet(width, height) },
			horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
			verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
			content = box(children, false, width - 20),
		}
	end

	-- The companies: each with its money and players, the one you play for
	-- first, with its colour and name to change; the others to join, with
	-- their password where they have one; a company of your own to found.
	-- Its head (tpf3mp/companies.lua, DECISIONS.md D22, proposed) also sets
	-- or takes away its password, sends players out, and opens or closes its
	-- stations to other companies' lines.
	local function companyRows(rows, status, shared, drafts)
		local roster = shared.companies
		if not roster then return end
		local companies = require("tpf3mp.companies")
		local function line(text) rows[#rows + 1] = label(text, "font-scale-annotation", COMPANIES_WIDTH - 40, 38) end
		if roster.passwordNotice then line(roster.passwordNotice) end
		local function row(children)
			local spaced = {}
			for i, child in ipairs(children) do
				if i > 1 then spaced[#spaced + 1] = gap(8) end
				spaced[#spaced + 1] = child
			end
			rows[#rows + 1] = box(spaced, true)
			rows[#rows + 1] = gap(8)
		end
		local function button(label, tooltip, onClick)
			return builtin.Button{ meta = { class = "secondary", tooltip = tooltip },
				content = builtin.TextView{ meta = { class = "font-scale-body" }, text = label }, onClick = onClick }
		end
		local function replacementOffer(offer)
			local ok, util = pcall(ug_require, "::/game_mechanics/finance/loan_util.tl")
			if not ok or type(util) ~= "table" then return nil end
			local make = util["create" .. tostring(offer.type) .. "Loan"]
			if type(make) ~= "function" then return nil end
			ok, util = pcall(make)
			if not ok or type(util) ~= "table" or util.type ~= offer.type
				or type(util.amount) ~= "number" or type(util.duration) ~= "number"
				or type(util.percentage) ~= "number" then return nil end
			return util
		end
		-- A field for a draft, sent with `act` on Enter or its button.
		local function field(draft, placeholder, secret, act)
			return builtin.TextInputField{
				meta = { class = "font-scale-body", styleSheet = sheet(300, 36) },
				placeholderText = placeholder,
				value = draft:get(),
				maxLength = 64,
				passwordMode = secret or nil,
				acceptOnFocusLoss = false,
				resetValueOnCancel = false,
				onTyping = function(text) draft:set(text) end,
				onCancel = function() shared.version = shared.version + 1 end,
				onValueChange = function(text) act(text) end,
			}
		end
		local function blank(text) return type(text) ~= "string" or text:match("^%s*$") end
		local names, byId, companyOf, mine = {}, {}, {}, nil
		for _, p in ipairs(status.players or {}) do byId[p.id] = p end
		for _, m in ipairs(roster.members) do companyOf[m.player] = m.company end
		for _, p in ipairs(status.players or {}) do
			local id = companyOf[p.id] or 0
			names[id] = names[id] or {}
			names[id][#names[id] + 1] = tostring(p.name) .. (p.me and " (you)" or "")
			if p.me then mine = id end
		end
		if mine == nil then mine = companyOf[status.me_id] or 0 end
		local ordered = {}
		for _, c in ipairs(roster.list) do if c.id == mine then ordered[#ordered + 1] = c end end
		for _, c in ipairs(roster.list) do if c.id ~= mine then ordered[#ordered + 1] = c end end
		local myName
		for _, c in ipairs(roster.list) do if c.id == mine then myName = c.name end end
		local iHead = status.me_id ~= nil and companies.head(roster, mine) == status.me_id
		local otherCards = {}
		for _, c in ipairs(ordered) do
			local head = companies.head(roster, c.id)
			local tags = {}
			if c.id == mine then tags[#tags + 1] = "Your company" end
			if head and byId[head] then tags[#tags + 1] = "head: " .. tostring(byId[head].name) end
			if c.locked then tags[#tags + 1] = "password" end
			if c.closed then tags[#tags + 1] = "stations closed" end
			local children = {}
			if c.id == mine then
				children[#children + 1] = builtin.ColorChooserButton{
					meta = { tooltip = "Your company's colour: its vehicles wear it" },
					colors = companyPalette(),
					color = vec3(c.color),
					onValueChange = function(v)
						local r, g, b = v.x or v[1], v.y or v[2], v.z or v[3]
						companyOp(shared, { Recolor = { company = c.id, color = { r = r, g = g, b = b } } },
							"Recolouring " .. c.name)
					end,
					resetButton = false,
				}
			end
			children[#children + 1] = gap(8)
			children[#children + 1] = label(c.name, "font-scale-title-4", COMPANIES_WIDTH - 100, 28)
			local card = { box(children, true), gap(6) }
			if #tags > 0 then card[#card + 1] = label(table.concat(tags, " · "), "font-scale-annotation, info", nil, 38) end
			if c.balance ~= nil then
				card[#card + 1] = label("Balance  " .. money(c.balance), "font-scale-body", nil, 38)
				if type(c.owed) == "number" and c.owed > 0 then
					card[#card + 1] = label("Debt  " .. money(c.owed), "font-scale-annotation", nil, 38)
				end
			end
			card[#card + 1] = gap(6)
			card[#card + 1] = label("Players  " .. (names[c.id] and table.concat(names[c.id], ", ") or "Nobody yet"),
				"font-scale-annotation", nil, 38)
			children = {}
			if c.id ~= mine then
				if c.locked then
					-- Its password, typed here; the room seals it, and only
					-- the seal reaches the games.
					children[#children + 1] = field(drafts.joinPassword, "Password", true, function(text)
						if blank(text) then return end
						companyOp(shared, { Join = c.id }, "Joining " .. c.name, nil, text)
						drafts.joinPassword:set("")
					end)
				end
				children[#children + 1] = button("Join", "Play for " .. tostring(c.name) .. " from now on"
					.. (c.locked and "; it needs its password" or ""), function()
					local password = c.locked and drafts.joinPassword:get() or nil
					if c.locked and blank(password) then
						shared.companyNote = c.name .. " needs its password"
						shared.version = shared.version + 1
						return
					end
					companyOp(shared, { Join = c.id }, "Joining " .. c.name, nil, password)
					drafts.joinPassword:set("")
				end)
			elseif c.id ~= 0 and #(names[c.id] or {}) <= 1 then
				-- Its last player dissolves it, once it owns nothing.
				children[#children + 1] = button("Dissolve", "Dissolve " .. tostring(c.name)
					.. " once it owns nothing, and play for the room's first company again",
					function() companyOp(shared, { Delete = c.id }, "Dissolving " .. c.name) end)
			end
			if #children > 0 then card[#card + 1] = gap(8); card[#card + 1] = box(children, true) end
			local destination = c.id == mine and rows or otherCards
			destination[#destination + 1] = box(card, false, COMPANIES_WIDTH - 24, 12, true)
			destination[#destination + 1] = gap(10)
		end
		line("MANAGE YOUR COMPANY")
		rows[#rows + 1] = gap(6)
		-- What the head of the player's company does with it.
		if iHead then
			local c
			for _, x in ipairs(roster.list) do if x.id == mine then c = x end end
			local lockChildren = {
				field(drafts.lockPassword, c.locked and "A new password" or "A password to join", true, function(text)
					if blank(text) then return end
					companyOp(shared, { Lock = c.id }, "Setting the password of " .. c.name, nil, text)
					drafts.lockPassword:set("")
				end),
				button(c.locked and "Change" or "Set", "Only players who know it can join " .. tostring(c.name),
					function()
						local text = drafts.lockPassword:get()
						if blank(text) then return end
						companyOp(shared, { Lock = c.id }, "Setting the password of " .. c.name, nil, text)
						drafts.lockPassword:set("")
					end),
			}
			if c.locked then
				lockChildren[#lockChildren + 1] = button("Remove", "Let anyone join " .. tostring(c.name),
					function() companyOp(shared, { Unlock = c.id }, "Removing the password of " .. c.name) end)
			end
			row(lockChildren)
			-- Who may add/change lines stopping at its stations (D22):
			-- a default, which also holds for companies founded later, and a
			-- choice for each other company, which wins over it. Per
			-- company, not per player: a company's players share everything
			-- it owns.
			row({ label("Station access applies to new and changed routes. Existing services keep running.",
				"font-scale-annotation", 500, 42) })
			local open = not c.closed
			row({
				label("Stations, by default and for companies founded later: " .. (open and "allowed" or "denied"),
					"font-scale-annotation", 300, 56),
				button(open and "Deny by default" or "Allow by default", open
					and "Keep " .. tostring(c.name) .. "'s stations from every company without a choice of its own"
					or "Let every company without a choice of its own stop at " .. tostring(c.name) .. "'s stations",
					function()
						companyOp(shared, { ShareStations = { company = c.id, open = not open } },
							(open and "Closing " or "Opening ") .. "the stations of " .. c.name .. " by default")
					end),
			})
			for _, other in ipairs(roster.list) do
				if other.id ~= c.id and not other.gone then
					local choice = companies.choice(c, other.id)
					local allowed = companies.lets(c, other.id)
					local children = {
						label(tostring(other.name) .. ": " .. (allowed and "allowed" or "denied")
							.. (choice == nil and " (default)" or ""), "font-scale-annotation", 300, 28),
						button(allowed and "Deny" or "Allow", (allowed and "Keep " or "Let ") .. tostring(other.name)
							.. (allowed and "'s lines from " or "'s lines stop at ") .. tostring(c.name) .. "'s stations",
							function()
								companyOp(shared, { StationAccess = { company = c.id, other = other.id, open = not allowed } },
									(allowed and "Denying " or "Allowing ") .. other.name)
							end),
					}
					if choice ~= nil then
						children[#children + 1] = button("Default", tostring(other.name) .. " follows the default again",
							function()
								companyOp(shared, { StationAccess = { company = c.id, other = other.id } },
									"Putting " .. other.name .. " back to the default")
							end)
					end
					row(children)
				end
			end
			for _, player in ipairs(companies.members(roster, mine)) do
				local p = byId[player]
				if player ~= status.me_id and p then
					row({
						label(p.name, nil, 300, 28),
						button("Send out", "Send " .. tostring(p.name) .. " back to the room's first company",
							function()
								companyOp(shared, { Dismiss = { company = c.id, player = player } },
									"Sending " .. tostring(p.name) .. " out of " .. c.name)
							end),
					})
				end
			end
		end
		row({
			field(drafts.rename, "A new name for " .. tostring(myName), false, function(text)
				if blank(text) then return end
				companyOp(shared, { Rename = { company = mine, name = text } }, "Renaming " .. tostring(myName))
				drafts.rename:set("")
			end),
			button("Rename", "Rename the company you play for", function()
				local text = drafts.rename:get()
				if blank(text) then return end
				companyOp(shared, { Rename = { company = mine, name = text } }, "Renaming " .. tostring(myName))
				drafts.rename:set("")
			end),
		})
		rows[#rows + 1] = gap(8)
		if #otherCards > 0 then
			line("OTHER COMPANIES")
			rows[#rows + 1] = gap(6)
			for _, card in ipairs(otherCards) do rows[#rows + 1] = card end
		end
		line("START ANOTHER COMPANY")
		rows[#rows + 1] = gap(6)
		row({
			field(drafts.found, "A company of your own", false, function(text)
				if blank(text) then return end
				companyOp(shared, { Create = { name = text } }, "Founding " .. text)
				drafts.found:set("")
			end),
			button("Found", "Found a company and play for it", function()
				local text = drafts.found:get()
				if blank(text) then return end
				companyOp(shared, { Create = { name = text } }, "Founding " .. text)
				drafts.found:set("")
			end),
		})
		-- Another company's loans are the room's (tpf3mp/companies.lua), on
		-- the terms the game offers; the first company's are in the game's
		-- own finance window.
		if mine ~= 0 then
			local loans = {}
			for _, loan in ipairs(roster.loans or {}) do if loan.company == mine then loans[#loans + 1] = loan end end
			for _, loan in ipairs(loans) do
				row({
					label("Loan: " .. money(loan.remaining) .. " owed of " .. money(loan.amount)
						.. ", " .. money(loan.payment) .. " a month, " .. (loan.months - loan.paid) .. " months left", nil, 300, 28),
					button("Repay", "Pay back what is still owed now", function()
						companyOp(shared, nil, "Repaying " .. money(loan.remaining), { Loan = { Repay = { loan = {
							type = "Custom", amount = loan.amount, duration = 1, percentage = 0, id = loan.id } } } })
					end),
				})
			end
			local offers = {}
			for _, offer in ipairs(roster.offers or {}) do
				if type(offer) == "table" and type(offer.amount) == "number" then
					offers[#offers + 1] = button("Borrow " .. money(offer.amount),
						string.format("Borrow %s with %g%% interest over the full term", money(offer.amount), (offer.percentage or 0) * 100),
						function()
							local nextOffer = replacementOffer(offer)
							if not nextOffer then
								shared.companyNote = "The game's loan utility could not draw the next " .. tostring(offer.type) .. " offer"
								shared.version = shared.version + 1
								return
							end
							local terms = { type = offer.type, amount = offer.amount, duration = offer.duration,
								percentage = offer.percentage }
							companyOp(shared, nil, "Borrowing " .. money(offer.amount),
								{ Loan = { Take = { next = nextOffer, offer = terms } } })
						end)
				end
			end
			for _, offer in ipairs(offers) do row({ offer }) end
		end
		if shared.companyNote then line(shared.companyNote) end
	end

	local function windowRows(status, draft, drafts)
		local shared = ui()
		local function send(text)
			local l = shared.link
			if not l or type(text) ~= "string" or text:match("^%s*$") then return end
			local ok, why = l:say(text)
			if ok then
				draft:set("")
			else
				shared.lines[#shared.lines + 1] = "(not sent: " .. tostring(why) .. ")"
			end
			shared.version = shared.version + 1
		end
		local rows = { label(status.room, "font-scale-title-3", 740, 36), gap(6) }
		-- The room's invite code alone (without the server the launcher may
		-- put before it), and Copy: the hook puts it on the clipboard, and
		-- the button says "Copied" for a moment.
		local code = type(status.invite) == "string" and status.invite:match("(%S+)%s*$")
		if code then
			rows[#rows + 1] = box({
				label("Invite code  " .. code, "font-scale-body"),
				builtin.Button{
					meta = { tooltip = "Copy the invite code, to paste it to your friends" },
					content = label((shared.copied or 0) > 0 and "Copied" or "Copy"),
					onClick = function()
						local l = shared.link
						local ok, why = false, "not linked"
						if l then ok, why = l:copy(code) end
						if ok then
							shared.copied = COPIED_FRAMES
						else
							shared.leaveNote = "Not copied: " .. tostring(why)
						end
						shared.version = shared.version + 1
					end,
				},
			}, true)
		end
		local statusRow = {}
		if status.speed then statusRow[#statusRow + 1] = label("Speed: " .. speedText(status.speed), "font-scale-annotation") end
		statusRow[#statusRow + 1] = gap(16)
		statusRow[#statusRow + 1] = label("Host controls speed", "font-scale-annotation")
		statusRow[#statusRow + 1] = gap(16)
		if status.diverged then
			statusRow[#statusRow + 1] = label("Resyncing your world", "font-scale-annotation, warning")
		else
			statusRow[#statusRow + 1] = label("Worlds match", "font-scale-annotation, success")
		end
		rows[#rows + 1] = box(statusRow, true)
		if status.diverged then
			rows[#rows + 1] = label("Your world differed at step " .. tostring(status.diverged)
				.. ". The room's world is on its way.", "font-scale-annotation, warning", 740, 64)
		end
		rows[#rows + 1] = gap(18)
		local players, online = {}, 0
		for _, p in ipairs(status.players or {}) do
			local tags = {}
			if p.owner then tags[#tags + 1] = "host" end
			if p.me then tags[#tags + 1] = "you" end
			if not p.connected then tags[#tags + 1] = "away" end
			if p.connected then online = online + 1 end
            local banners = require("tpf3mp.banners")
            local stage = banners.stage(p, true)
            if stage then tags[#tags + 1] = stage end
            players[#players + 1] = builtin.ImageView{
                meta = { styleSheet = sheet(PLAYERS_WIDTH - 32, 40) }, path = banners.picture(banners.of(p)),
            }
            local portrait = banners.portraitOf(p)
            if portrait then
                players[#players + 1] = builtin.ImageView{
                    meta = { styleSheet = sheet(40, 40) }, path = portrait,
                }
            end
			players[#players + 1] = box({
				label(p.name, "font-scale-body", PLAYERS_WIDTH - 40, 16),
				label(#tags > 0 and table.concat(tags, " · ") or "playing", "font-scale-annotation" .. (p.me and ", info" or ""), nil, 16),
			}, false, PLAYERS_WIDTH - 24, 10, true)
			players[#players + 1] = gap(8)
		end
		local companies = {}
		companyRows(companies, status, shared, drafts)
		if #companies == 0 then companies[1] = label("Waiting for the companies...", "font-scale-annotation") end
		rows[#rows + 1] = box({
			box({ label("Players", "font-scale-title-4"),
				label(online .. " of " .. #(status.players or {}) .. " online", "font-scale-annotation"), gap(10),
				scroll(players, PLAYERS_WIDTH, BODY_HEIGHT) }, false, PLAYERS_WIDTH),
			gap(20),
			box({ label("Companies", "font-scale-title-4"),
				label("Choose who you build with", "font-scale-annotation"), gap(10),
				scroll(companies, COMPANIES_WIDTH, BODY_HEIGHT) }, false, COMPANIES_WIDTH),
		}, true)
		rows[#rows + 1] = gap(18)
		rows[#rows + 1] = label("Chat", "font-scale-title-4")
		rows[#rows + 1] = gap(6)
		local chat = {}
		-- The newest lines, in a bounded panel. The composer stays visible.
		for i = math.max(1, #shared.lines - CHAT_SHOWN + 1), #shared.lines do
			chat[#chat + 1] = label(shared.lines[i], "font-scale-body", 720, 64)
			chat[#chat + 1] = gap(4)
		end
		if #chat == 0 then chat[1] = label("Nobody said anything yet.", "font-scale-annotation") end
		rows[#rows + 1] = scroll(chat, 760, 92)
		rows[#rows + 1] = gap(8)
		rows[#rows + 1] = box({
				builtin.TextInputField{
					meta = { class = "font-scale-body", styleSheet = sheet(652, 36) },
					placeholderText = "Say something to the room",
					value = draft:get(),
					maxLength = 280,
					acceptOnFocusLoss = false,
					-- Clicking away keeps what was typed, and a redraw shows
					-- it: Send sends what the field shows, never a line the
					-- field dropped (it emptied itself on a cancel, build
					-- 40408, while Send still had the text).
					resetValueOnCancel = false,
					onTyping = function(text) draft:set(text) end,
					onCancel = function() shared.version = shared.version + 1 end,
					onValueChange = function(text) send(text) end,
				},
				gap(8),
				builtin.Button{
					meta = { class = "primary", styleSheet = sheet(100, 36) },
					content = label("Send"),
					onClick = function() send(draft:get()) end,
				},
			}, true)
		return rows
	end

	-- The Multiplayer window, which the game's window container shows, as
	-- the game bar shows its context help (game_bar.tl: the window API's
	-- addSingletonWindow; a window rendered anywhere else shows nothing,
	-- build 40408). One recipe for both plugins, kept in ui(): the game may
	-- run this file once for each.
	local function windowRecipe()
		local shared = ui()
		if shared.window == nil then
			shared.window = react.RegisterWrapperRecipe("Tpf3mpWindow", builtin.Window, function(params)
				local drawn = react.useState(0)
				local draft = react.useRef("")
				local drafts = { rename = react.useRef(""), found = react.useRef(""),
					joinPassword = react.useRef(""), lockPassword = react.useRef("") }
				react.onStep(function()
					local version = ui().version
					if version ~= drawn:old() then drawn:set(version) end
				end)
				local _ = drawn:old()
				local status = ui().status
				local rows
				if status then
					rows = windowRows(status, draft, drafts)
				else
					rows = { builtin.TextView{ text = "Not in a room." } }
				end
				return builtin.Window{
					id = "tpf3mp.multiplayer.window",
					title = "Multiplayer",
					closable = true,
					onClose = params.onClose,
					-- Where it opens, as a share of the screen (the entity
					-- windows open at 1, 0: top right): at the left, below
					-- the mods' buttons, which it would cover at 0, 0.
					initialX = 0,
					initialY = 0.15,
					content = box(rows, false, WINDOW_WIDTH, 20, true),
				}
			end)
		end
		return shared.window
	end

	-- Opens the Multiplayer window, or closes it if open. A window the game
	-- will not show is said in the game bar and the log.
	local function toggleWindow()
		local shared = ui()
		local ok, why = pcall(function()
			local windows = game_react_globals.getDefaultWindowApi()
			local recipe = windowRecipe()
			local function close()
				shared.open = false
				shared.version = shared.version + 1
				windows.removeAllWindows(recipe)
			end
			if shared.open then
				close()
			else
				shared.open = true
				shared.unread = 0
				shared.version = shared.version + 1
				windows.addSingletonWindow(recipe, { onClose = close })
				windows.moveSingletonWindowToFront(recipe)
			end
		end)
		if not ok then
			shared.open = false
			shared.version = shared.version + 1
			say("the Multiplayer window did not open: " .. tostring(why))
			notice = "The Multiplayer window did not open"
		end
	end

	-- The proposal another member's preview `action` would build here, for
	-- the company `from` plays for (tpf3mp/apply.lua, apply.proposalOf), its
	-- context and that company; or nil and why.
	local function previewProposal(action, from)
		local shared = ui()
		local roster = shared.companies
		local company = require("tpf3mp.follow").companyOf(roster, from)
		local proposal, context = require("tpf3mp.apply").proposalOf(action, { company = company, roster = roster })
		if proposal == nil then return nil, context end
		return proposal, context, company
	end

	-- Has the hook draw another member's preview `kept` (its proposal and
	-- context), or with nil clear it (tpf3mp/previews.lua). Answers true and
	-- the game's ProposalData for it, or nil and why.
	local function drawPreview(from, kept)
		if kept == nil then
			link:undrawPreview(from)
			return true
		end
		return link:drawPreview(from, kept.proposal, kept.context, function(proposal, context)
			return api.engine.util.proposal.makeProposalData(proposal, context)
		end)
	end

	local Tpf3mpPlugin = react.RegisterPluginRecipe(game_bar_widgets.GameBarInfoDisplayExtension, "Tpf3mpPlugin", function()
		-- Once per game: the ref lives as long as this plugin is mounted.
		local started = react.useRef(false)
		-- The refusal notice shown, or false, and the frames it has left.
		local shown = react.useState(false)
		local frames = react.useRef(0)
		-- The room's version drawn, and the one last seen: a change redraws.
		local room = react.useState(0)
		local seen = react.useRef(0)
		react.onStep(function()
			if not started:get() then
				started:set(true)
				-- A new world's window container has no Multiplayer window.
				ui().open = false
				local ok, err = pcall(start)
				if not ok then say("start failed: " .. tostring(err)) end
			end
			local ok, err = pcall(serve)
			if not ok then say("serving the hook failed: " .. tostring(err)) end
			local followed, why = pcall(follow)
			if not followed then say("reading the room failed: " .. tostring(why)) end
			local copied = ui().copied
			if copied and copied > 0 then
				ui().copied = copied - 1
				if copied == 1 then ui().version = ui().version + 1 end
			end
			if ui().version ~= seen:get() then
				seen:set(ui().version)
				room:set(ui().version)
			end
			runPending()
			-- The other members' build previews, made into the proposals
			-- they would build here and drawn by the hook (tpf3mp/previews.lua;
			-- never the game's ProposalViewer, which build 40408 allows only
			-- inside a tool's ActionDescriptor: a fatal assert elsewhere).
			-- Out of a room too: its end tells each one drawn as gone.
			if link then
				local took, why = pcall(require("tpf3mp.previews").take, link, previewProposal, drawPreview)
				if not took then say("taking the build previews failed: " .. tostring(why)) end
			end
			if link and guardedCmd then
				local delivered, why = pcall(function()
					local results = link:results()
					-- The answers to the HUD's state's commands go on to it
					-- (tpf3mp/hudguard.lua), which waits for them there.
					pcall(require("tpf3mp.hudguard").forward, link, results)
					require("tpf3mp.guard").deliver(guardedCmd, results, sees, clock)
					local shared = ui()
					for _, r in ipairs(results or {}) do
						local doing = shared.asked and r.ticket and shared.asked[r.ticket]
						if doing then
							shared.asked[r.ticket] = nil
							shared.companyNote = r.ok and (doing .. ": done")
								or (doing .. ": not done, " .. tostring(r.why))
							shared.version = shared.version + 1
						end
						local asked = r.ticket and answers[r.ticket]
						if asked then
							answers[r.ticket] = nil
							if r.ok ~= true then notice = asked .. ": not done, " .. tostring(r.why) end
						end
					end
				end)
				if not delivered then say("answering the player's commands failed: " .. tostring(why)) end
			end
			if link then
				local told = link:note("tpf3mp.notice")
				if toldNotice == false then
					toldNotice = told or ""
				elseif told and told ~= toldNotice then
					toldNotice = told
					local text = told:match("^%d+	(.+)$")
					if text then notice = text end
				end
			end
			if notice then
				shown:set(notice)
				frames:set(NOTICE_FRAMES)
				notice = nil
			elseif frames:get() > 0 then
				frames:set(frames:get() - 1)
				if frames:get() == 0 then shown:set(false) end
			end
		end)
		-- Even empty, the layout keeps the plugin mounted, so onStep keeps
		-- running.
		local children = {}
		local _ = room:old()
		local shared = ui()
		if shared.status then
			local label = summary(shared.status)
			if shared.unread > 0 then label = label .. " · " .. shared.unread .. " new" end
			children[#children + 1] = builtin.Button{
				meta = { tooltip = "Open the Multiplayer window" },
				-- The game bar is low: the small font keeps the button in it.
				content = builtin.TextView{ meta = { class = "font-scale-annotation" }, text = label },
				onClick = toggleWindow,
			}
		end
		if shown:old() then
			children[#children + 1] = builtin.TextView{ text = shown:old() }
		end
		return builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = children,
		}
	end)

	-- The Multiplayer button in the game's area for mods' buttons, in the
	-- room's game.
	local Tpf3mpButton = react.RegisterPluginRecipe(main_mod_button_area.MainModButtonAreaExtension, "Tpf3mpButton", function()
		local drawn = react.useState(0)
		react.onStep(function()
			local version = ui().version
			if version ~= drawn:old() then drawn:set(version) end
		end)
		local _ = drawn:old()
		local shared = ui()
		local children = {}
		if shared.status then
			children[1] = builtin.Button{
				meta = { tooltip = "Multiplayer: the room, its players and its chat" },
				content = builtin.TextView{
					text = "Multiplayer" .. (shared.unread > 0 and (" (" .. shared.unread .. ")") or ""),
				},
				onClick = toggleWindow,
			}
		end
		return builtin.BoxLayout{ orientation = builtin.type.Orientation.Horizontal, children = children }
	end)

	return {
		Tpf3mpPlugin = Tpf3mpPlugin,
		Tpf3mpButton = Tpf3mpButton,
	}
end
