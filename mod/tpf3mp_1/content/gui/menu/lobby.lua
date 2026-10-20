-- The Multiplayer window's content, on the game's main menu (docs/LOBBY.md),
-- and the live lines of the menu's Multiplayer cards.
--
-- It shows the lobby the hook hands it and sends the player's choices back,
-- over the hook's request channel: `resolveutil.loadfile("tpf3mp_1::/tpf3mp/state.lua")`
-- answers with the launcher's lobby as a Lua table literal; an action is left as JSON in
-- `resolveutil.__tpf3mp_action` and then `resolveutil.loadfile("tpf3mp_1::/tpf3mp/act.lua")`
-- is called, which the hook answers after taking the JSON (the loader accepts
-- exactly one argument): "ok", or "error: " and why. Both files exist in the
-- mod, because the game checks that before it lets the loader run; the hook
-- answers before the loader reads them, so their contents never matter.
-- Without a hook (a game Steam started) this window is not reachable at all.
--
-- The hook passes everything on to the TPF3-MP launcher that started the
-- game, over its link (D17): the launcher connects, makes and joins rooms and
-- carries the chat; the window shows what the launcher says, a few times a
-- second. Connecting goes to the launcher's own server (D12). The server
-- lists no rooms: a room is joined by the invite its owner sends.
--
-- Plain Lua, loaded by the mod's main_page.tl through `ug_require`; it uses
-- the same react, builtin and helpers the menu does, the menu's own classes
-- (primary and secondary buttons, the font-scale-* sizes, the default style
-- sheet's colours and tapes: success, warning, error, info) and icons, so it
-- looks like the rest of the menu. crates/tpf3mp-hook/src/lobby.rs renders it
-- against a stand-in for the game's GUI (tests/lua/fake_menu.lua) in every
-- state and clicks its buttons.

local react = ug_require "::/gui/main/react.lua"
local builtin = ug_require "::/gui/main/builtin.lua"
local gui_react_util = ug_require "::/gui/main/gui_react_util.tl"
local button_react_util = ug_require "::/gui/main/button_react_util.tl"
local menu_icon_react_util = ug_require "::/gui/menu/menu_icon_react_util.tl"
local content_card = ug_require "::/gui/main/content_card.tl"
local tile_list_react_util = ug_require "::/gui/main/tile_list_react_util.tl"
local mod_manager_react_util = ug_require "::/gui/menu/mod_manager_react_util.tl"
local roommods = ug_require "tpf3mp_1::/gui/menu/roommods.lua"

local lobby = {}

-- Stock world setup reads this selection. Entering it from a room must
-- include the multiplayer script, while preserving the player's mods.
function lobby.prepareNewWorld()
	local config = api.type.AppConfig.new(api.util.getAppConfig())
	local menu = {}
	for key, value in pairs(config.mainMenuState or {}) do menu[key] = value end
	local mods, found = {}, false
	for _, name in ipairs(menu.activeModsState or {}) do
		mods[#mods + 1] = name
		if name == "tpf3mp_1" then found = true end
	end
	if not found then mods[#mods + 1] = "tpf3mp_1" end
	menu.activeModsState = mods
	config.mainMenuState = menu
	api.util.setAppConfig(config, false)
end

local ICON = {
	ready = "::/gui/menu/icons/symbol_check.tga",
	host = "::/gui/menu/icons/symbol_crown_laurels.tga",
	away = "::/gui/menu/icons/symbol_x.tga",
	lock = "::/gui/menu/icons/symbol_lock_unlocked.tga",
	player = "::/gui/menu/icons/profile.tga",
	alert = "::/gui/menu/icons/alert.tga",
	kick = "::/gui/menu/icons/trash.tga",
	loading = "::/gui/menu/icons/loading.tga",
	save = "::/gui/menu/icons/load_game.tga",
	multiplayer = "tpf3mp_1::/gui/tpf3mp/icons/menu_multiplayer_50.tga",
}

-- The page's content size and the two columns of each view: the game's own
-- card on its menu pages, as its Load Game page has it (main_menu_sizes.lua:
-- 1646 by 774, less the card's and its body's padding and the header).
local WIDTH, HEIGHT = 1600, 700
local LEFT, RIGHT = 880, 660
-- The room tab's columns (left as the Load Game page's three fifths), the
-- world's picture, the players' and the chat's scrolled heights.
-- A mod's tile, as the game's mod selector shows one, five to a row, and
-- how high the mod tabs scroll their tiles. (One table: the window's
-- function may hold only so many upvalues.)
local SIZE = {
	ROOM_LEFT = 670, ROOM_RIGHT = 860,
	PREVIEW_HEIGHT = 262, PLAYERS_HEIGHT = 190,
	TILE_WIDTH = 280, TILE_HEIGHT = 158, TILE_COLUMNS = 5,
	MODS_HEIGHT = 450,
}
local MOD_PLACEHOLDER = "::/gui/menu/images/mod_placeholder.tga"
local FIELD = 400
-- Player counts a room can be created for, as the launcher offers them.
local MIN_PLAYERS, MAX_PLAYERS, DEFAULT_PLAYERS = 2, 16, 4
-- How often the window asks the hook for the lobby, in seconds, and how
-- many of those asks an action is shown as under way at most.
local POLL = 0.4
local PENDING_POLLS = 20
-- Polls a Mod Hub lookup, or a subscription Mod Hub has not taken yet, waits
-- for Mod Hub's answer before it fails: 30 seconds.
local HUB_POLLS = 75
-- How many polls Copy says "Copied" for: about two seconds.
local COPIED_POLLS = 5
-- How many polls the launcher's latest notice shows for: about eight
-- seconds. Its errors show until they are gone.
local NOTICE_POLLS = 20

-- Whether the save `name` is one of the hook's own copies of a room's world
-- (docs/HOOKS.md, "The room's world"), which it removes once their game
-- ends: never a world to start a room from.
local function plainUint(value, maximum)
	return value ~= nil and value:match("^%d+$") ~= nil
		and (value == "0" or value:sub(1, 1) ~= "0")
		and (#value < #maximum or (#value == #maximum and value <= maximum))
end
local function hookCopy(name)
	local pid = name:match("^tpf3mp_room_(%d+)$")
	if pid then return pid ~= "0" and plainUint(pid, "4294967295") end
	local owner, event = name:match("^tpf3mp_(%d+)_(%d+)$")
	return owner ~= "0" and plainUint(owner, "4294967295")
		and plainUint(event, "18446744073709551615")
end

-- The request channel -------------------------------------------------------

local function say(line)
	pcall(debugPrint, "[tpf3mp] lobby: " .. line)
end

local function ask(request)
	if type(resolveutil) ~= "table" or type(resolveutil.loadfile) ~= "function" then
		return nil, "no loader"
	end
	local ok, reply = pcall(resolveutil.loadfile, "tpf3mp_1::/tpf3mp/" .. request .. ".lua")
	if not ok then
		return nil, tostring(reply)
	end
	if type(reply) ~= "string" then
		return nil, "no hook answered"
	end
	return reply
end

-- A table literal in an empty environment: Lua 5.2's load, or 5.1's.
local function evaluate(text)
	local chunk, err
	if setfenv then
		chunk, err = loadstring(text, "=tpf3mp-state")
		if chunk then setfenv(chunk, {}) end
	else
		chunk, err = load(text, "=tpf3mp-state", "t", {})
	end
	if not chunk then
		return nil, err
	end
	local ok, value = pcall(chunk)
	if not ok then
		return nil, value
	end
	return value
end

local lastProblem = nil
local function fetchState()
	local reply, why = ask("state")
	if not reply then
		if why ~= lastProblem then
			lastProblem = why
			say("state request failed: " .. tostring(why))
		end
		return nil, why
	end
	local state, err = evaluate("return " .. reply)
	if type(state) ~= "table" then
		return nil, "bad state: " .. tostring(err or state)
	end
	return state, nil, reply
end
lobby.fetchState = fetchState

local function jsonString(text)
	text = tostring(text or "")
	return '"' .. text:gsub('[%c"\\]', function(c)
		if c == '"' then return '\\"' end
		if c == "\\" then return "\\\\" end
		if c == "\n" then return "\\n" end
		if c == "\r" then return "\\r" end
		if c == "\t" then return "\\t" end
		return string.format("\\u%04x", c:byte())
	end) .. '"'
end

-- `value` as JSON: a boolean, a whole number, a string, a list (a table
-- with items at 1.., or an empty one) or an object (string keys, sorted).
local function jsonValue(value)
	if type(value) == "boolean" then
		return value and "true" or "false"
	elseif type(value) == "number" then
		return string.format("%d", value)
	elseif type(value) == "table" then
		local parts = {}
		if #value > 0 or next(value) == nil then
			for _i, item in ipairs(value) do parts[#parts + 1] = jsonValue(item) end
			return "[" .. table.concat(parts, ",") .. "]"
		end
		local keys = {}
		for key in pairs(value) do keys[#keys + 1] = tostring(key) end
		table.sort(keys)
		for _i, key in ipairs(keys) do
			parts[#parts + 1] = jsonString(key) .. ":" .. jsonValue(value[key])
		end
		return "{" .. table.concat(parts, ",") .. "}"
	end
	return jsonString(value)
end
lobby.jsonValue = jsonValue

-- Sends one action, given as a table with an `action` field and its fields.
-- Returns nil when the hook took it, or why it did not.
local function act(fields)
	local json = jsonValue(fields)
	if type(resolveutil) ~= "table" then
		say("action " .. tostring(fields.action) .. " not sent: no loader")
		return "the game's loader is not there"
	end
	resolveutil.__tpf3mp_action = json
	local reply, why = ask("act")
	resolveutil.__tpf3mp_action = nil
	if not reply then
		say("action " .. tostring(fields.action) .. " not sent: " .. tostring(why))
		return "not sent: " .. tostring(why)
	elseif reply ~= "ok" then
		say("action " .. tostring(fields.action) .. ": " .. reply)
		return (reply:gsub("^error: ", ""))
	end
	return nil
end

-- What the lobby says, in words -----------------------------------------------

local function sizeText(bytes)
	bytes = tonumber(bytes) or 0
	if bytes >= 1e9 then return string.format("%.1f GB", bytes / 1e9) end
	if bytes >= 1e6 then return string.format("%.1f MB", bytes / 1e6) end
	if bytes >= 1e3 then return string.format("%d kB", math.floor(bytes / 1e3 + 0.5)) end
	return string.format("%d B", bytes)
end

-- The server's name as the window shows it: never its address. The
-- launcher names its default server ("EU"); any other is "another server",
-- as a name that looks like host:port or an IP address is.
local function serverName(state)
	local name = state.server
	local elsewhere = state.server_default and state.server_default ~= ""
		and state.server_address and state.server_address ~= state.server_default
	if elsewhere or type(name) ~= "string" or name == "" then
		return elsewhere and _("another server") or _("the TPF3-MP server")
	end
	if name:find(":%d+$") or name:find("^%d+%.%d+%.%d+%.%d+") or name:find("^%[") then
		return _("another server")
	end
	return name
end
lobby.serverName = serverName

-- The release's servers the room list comes from, as "EU (24 ms) and US
-- (110 ms)"; nil when it comes from one server.
function lobby.listedServers(list)
	local servers = list and list.servers or {}
	if #servers < 2 then return nil end
	local named = {}
	for _i, server in ipairs(servers) do
		local ping = tonumber(server.ping) or 0
		if not server.reachable then
			named[#named + 1] = string.format(_("%s (not answering)"), server.name)
		elseif ping > 0 then
			named[#named + 1] = string.format(_("%s (%d ms)"), server.name, ping)
		else
			named[#named + 1] = server.name
		end
	end
	if #named == 2 then return string.format(_("%s and %s"), named[1], named[2]) end
	return table.concat(named, ", ")
end

-- `text` with any server address in it (an IP address, host:port) put as
-- "the server": the window names servers, never their addresses.
local function hideAddress(text)
	if type(text) ~= "string" then return text end
	text = text:gsub("%[[%x:]+%]:%d+", _("the server"))
	text = text:gsub("%d+%.%d+%.%d+%.%d+:%d+", _("the server"))
	text = text:gsub("%d+%.%d+%.%d+%.%d+", _("the server"))
	text = text:gsub("[%w%-]+%.[%w%.%-]+:%d+", _("the server"))
	text = text:gsub("localhost:%d+", _("the server"))
	return text
end
lobby.hideAddress = hideAddress

-- The room's six-character invite code (D13); region routing stays in the
-- release's trusted launcher UI, never in the copied code.
local function inviteCode(invite)
	if type(invite) ~= "string" then return "" end
	return invite:match("(%S+)%s*$") or invite
end

local function you(room)
	for _i, member in ipairs(room and room.members or {}) do
		if member.you then return member end
	end
	return nil
end

local function readyCount(room)
	local count = 0
	for _i, member in ipairs(room.members or {}) do
		if member.ready then count = count + 1 end
	end
	return count
end

local function everyoneReady(room)
	return #(room.members or {}) > 0 and readyCount(room) == #room.members
end

-- The room's world in this game, in words, and how far it is (0 to 1), or
-- nil when there is none of the room's.
local function worldText(state)
	if not state.room then return nil end
	if state.world == "fetching" then
		local total = tonumber(state.total) or 0
		local bytes = tonumber(state.bytes) or 0
		if total > 0 then
			local done = math.min(1, bytes / total)
			return string.format(_("Receiving the room's world: %d%% (%s of %s)"),
				math.floor(done * 100), sizeText(bytes), sizeText(total)), done
		end
		return _("Receiving the room's world..."), 0
	elseif state.world == "loading" then
		return _("Loading the room's world..."), 1
	elseif state.world == "playing" then
		return _("Playing the room's game"), 1
	end
	return nil
end
lobby.worldText = worldText

-- Where the player is, for the Multiplayer cards on the main menu: a line
-- under the card's title.
function lobby.summary(state)
	if not state then
		return _("Play together online")
	end
	if not state.linked then
		return _("Start the game from the TPF3-MP launcher")
	end
	if state.connection == "connecting" then
		return _("Connecting...")
	end
	if state.connection ~= "connected" then
		return _("Play together online")
	end
	local room = state.room
	if not room then
		return string.format(_("Online on %s"), serverName(state))
	end
	local text = worldText(state)
	if text then return text end
	return string.format(_("%s · %d/%d players · %d ready"), room.name, #room.members,
		room.max_players, readyCount(room))
end

-- Styling ---------------------------------------------------------------------

-- An inline style sheet: size as {w, h}, padding as {top, right, bottom,
-- left}. Those are what api.gui.StyleSheet offers (no margin, no minSize), so
-- spacing between elements is done with gap() below. A size of -1 leaves
-- that side to the content, as the game's style sheets write it; a size of
-- 0 is a size of nothing, and hides everything inside (the game drew the
-- create and join columns, sized {w, 0}, as two thin bars).
local AUTO = -1
local function style(t)
	local s = api.gui.StyleSheet.new()
	if t.size then s.size = api.type.Vec2f.new(t.size[1], t.size[2]) end
	if t.padding then s.padding = api.type.Vec4f.new(t.padding[1], t.padding[2], t.padding[3], t.padding[4]) end
	return s
end

local function gap(px)
	px = math.max(1, px or 12)
	return builtin.Component{
		meta = { styleSheet = style{ size = { px, px } } },
		mouseTransparent = true,
		layout = builtin.BoxLayout{ children = {} },
	}
end

-- A row or column's children with a gap between each pair.
local function spaced(children, px)
	local out = {}
	for i, child in ipairs(children) do
		if i > 1 then out[#out + 1] = gap(px or 8) end
		out[#out + 1] = child
	end
	return out
end

local function label(text, class, sheet)
	return builtin.TextView{
		meta = { class = class or "font-scale-body", styleSheet = sheet },
		text = text or "",
	}
end

-- A smaller, quieter line, as the menu's cards write their descriptions.
local function note(text, class)
	return label(text, "font-scale-annotation" .. (class and (", " .. class) or ""))
end

-- A word on a coloured tape, as the game marks states: success, warning,
-- error or info.
local function badge(text, tone)
	return label(" " .. text .. " ", "font-scale-annotation, " .. (tone or "info") .. "-tape")
end

local function icon(path, px)
	px = px or 20
	return builtin.ImageView{
		meta = { styleSheet = style{ size = { px, px } } },
		path = path,
		scaling = builtin.type.ImageViewScaling.AutoFit,
	}
end

local function row(children, sheet)
	return builtin.Component{
		meta = { styleSheet = sheet },
		mouseTransparent = true,
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = children,
		},
	}
end

local function column(children, sheet)
	return builtin.Component{
		meta = { styleSheet = sheet },
		mouseTransparent = true,
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Vertical,
			children = children,
		},
	}
end

local function button(text, onClick, class, enabled, tooltip)
	return builtin.Button{
		meta = {
			class = class or "secondary",
			enabled = enabled ~= false,
			tooltip = tooltip,
		},
		content = builtin.TextView{ meta = { class = "font-scale-body" }, text = text },
		onClick = onClick,
	}
end

local function primary(text, onClick, enabled, tooltip)
	return button(text, onClick, "primary", enabled, tooltip)
end

local function input(ref, placeholder, width, params)
	params = params or {}
	return builtin.TextInputField{
		meta = { class = "font-scale-body", styleSheet = style{ size = { width or FIELD, 36 } } },
		value = ref:get(),
		placeholderText = placeholder,
		passwordMode = params.password or false,
		maxLength = params.maxLength,
		acceptOnFocusLoss = params.acceptOnFocusLoss ~= false,
		resetValueOnCancel = false,
		onValueChange = function(value)
			ref:set(value)
			if params.onEnter then params.onEnter(value) end
		end,
		onTyping = function(value) ref:set(value) end,
	}
end

local function field(caption, ref, placeholder, params)
	return column({
		note(caption),
		gap(4),
		input(ref, placeholder, FIELD, params),
		gap(12),
	}, style{ size = { FIELD, 76 } })
end

-- A choice from a list: `items` as { value, text }.
local function choice(caption, value, items, onChange, explain)
	local entries = {}
	for _i, item in ipairs(items) do
		entries[#entries + 1] = builtin.ComboBoxItem{
			value = item[1],
			content = builtin.TextView{ meta = { class = "font-scale-body" }, text = item[2] },
		}
	end
	local children = {
		note(caption),
		gap(4),
		builtin.Component{
			meta = { styleSheet = style{ size = { FIELD, 36 } } },
			layout = builtin.BoxLayout{
				children = {
					builtin.ComboBox{ value = value, items = entries, onValueChange = onChange },
				},
			},
		},
	}
	if explain then
		children[#children + 1] = gap(4)
		children[#children + 1] = note(explain)
	end
	children[#children + 1] = gap(12)
	return column(children, style{ size = { FIELD, explain and 94 or 76 } })
end

local function heading(text, sub)
	local children = { label(text, "font-scale-title-3") }
	if sub then
		children[#children + 1] = gap(2)
		children[#children + 1] = note(sub)
	end
	children[#children + 1] = gap(12)
	return column(children)
end

-- The room browser --------------------------------------------------------------

-- Cards a row of the room list holds, and a card's size: the game's
-- new-game climate cards (`small-rectangle-card`, 316 by 181) a little
-- smaller, three to the window's width.
local CARDS_PER_ROW = 3
local CARD_WIDTH, CARD_HEIGHT = 272, 156
-- The first page's two cards, Join and Host.
local CHOICE_WIDTH, CHOICE_HEIGHT = 768, 500
-- Polls between two asks for the room list while it is shown.
local LIST_POLLS = 25

-- The game's own card button and label, as its main menu builds them; nil
-- if the game has none, and a plain button with a picture is drawn instead.
local cards = (function()
	local ok, util = pcall(ug_require, "::/gui/menu/menu_icon_react_util.tl")
	if ok and type(util) == "table" and util.CardButton and util.makeCardLabelBottomComponent then
		return util
	end
	return nil
end)()

-- The game's pictures of each climate on its main menu's New Game card,
-- for a climate whose own description has no picture.
local CLIMATE_PICTURES = {
	temperate = "::/gui/menu/images/temperate_ingame.tga",
	subarctic = "::/gui/menu/images/subarctic_ingame.tga",
	tropical = "::/gui/menu/images/tropical_ingame.tga",
	dry = "::/gui/menu/images/dry_ingame.tga",
}
local UNKNOWN_PICTURE = "::/gui/menu/images/m05_ingame.tga"

-- The game's description of the climate `map` names (`temperate`), if it
-- has one: what its New Game page shows, its name and its picture.
local function climate(map)
	if type(map) ~= "string" or map == "" then return nil end
	local ok, desc = pcall(function()
		local rep = app.res.climateRep
		local id = rep.find("::/climates/" .. map .. "/" .. map .. ".clima")
		if id == nil or id < 0 then return nil end
		return rep.get(id).desc
	end)
	return ok and desc or nil
end

-- The climate's name, as players read it.
function lobby.climateName(map)
	local desc = climate(map)
	if desc and type(desc.name) == "string" and desc.name ~= "" then return desc.name end
	if type(map) ~= "string" or map == "" then return _("Unknown map") end
	return (map:gsub("^%l", string.upper))
end

-- The picture of the climate `map` names.
function lobby.climatePicture(map)
	local desc = climate(map)
	if desc and type(desc.icon) == "string" and desc.icon ~= "" then return desc.icon end
	return CLIMATE_PICTURES[map] or UNKNOWN_PICTURE
end

-- A save's climate and year, as the game's Load Game page reads them
-- (savegame_react_util.tl): the save's configDict "climate" and its
-- metadata's date. Read once, in the background (app.getSavegameInfo);
-- until then, or when the game cannot say, the map is "" and the year 0.
local saveDetailsRead = {}
local function yearOf(metadata)
	local ok, year = pcall(function() return api.type.Date.new(metadata.date).year end)
	if ok and type(year) == "number" and year > 1000 and year < 3000 then return math.floor(year) end
	if type(metadata.startYear) == "number" and metadata.startYear > 1000 then return math.floor(metadata.startYear) end
	return 0
end
function lobby.saveDetails(name)
	local read = saveDetailsRead[name]
	if not read then
		read = { map = "", year = 0 }
		saveDetailsRead[name] = read
		local ok, async = pcall(function()
			local namespace = app.SaveGameNamespace.getSavegame()
			for _i, info in ipairs(app.findAllSavegames(namespace) or {}) do
				if info.saveName == name or info.saveName == name .. ".sav" then
					local id = api.type.SavegameId.new()
					id.path = info.path
					id.saveGameName = info.saveName
					id.saveGameNamespace = namespace
					return app.getSavegameInfo(id)
				end
			end
			return nil
		end)
		read.async = ok and async or nil
		if not ok then say("the save " .. tostring(name) .. " could not be read: " .. tostring(async)) end
	end
	if read.async then
		local ok, done = pcall(function() return read.async:isCompleted() end)
		if ok and done then
			local got, data = pcall(function() return read.async:get() end)
			read.async = nil
			if got and data and data.info then
				for _i, pair in ipairs(data.info.configDict or {}) do
					if pair[1] == "climate" and type(pair[2]) == "string" then
						read.map = pair[2]:match("([%w_]+)%.clima$") or pair[2]
					end
				end
				if data.info.metadata then read.year = yearOf(data.info.metadata) end
				-- Its picture, as the Load Game page's tile shows it.
				pcall(function()
					local shot = data.info.metadata.screenshot
					read.shot = { data = shot.image_native:clone(), width = shot.width, height = shot.height }
				end)
			end
		elseif not ok then
			read.async = nil
		end
	end
	return read
end

-- The save a room starts from, in a line: its name, its map and year when
-- the room knows them, and whether the room has it yet.
function lobby.startLine(start)
	local parts = { start.name }
	if type(start.map) == "string" and start.map ~= "" then parts[#parts + 1] = lobby.climateName(start.map) end
	if (tonumber(start.year) or 0) > 0 then parts[#parts + 1] = tostring(start.year) end
	if not start.arrived then parts[#parts + 1] = _("on its way to the room") end
	return table.concat(parts, " · ")
end

-- Whether the room's game must wait for its start save: the owner's still
-- going up, or the room not having it yet.
function lobby.startWaits(room)
	return room.upload ~= nil or (room.start ~= nil and not room.start.arrived)
end

-- Polls a pick of the room's start save waits at most for the game to read
-- the save's map and year.
local PICK_POLLS = 8

-- Banners ---------------------------------------------------------------------

local banners = ug_require "tpf3mp_1::/scripts/tpf3mp/banners.lua"
local BANNERS = banners.LIST
lobby.BANNERS = BANNERS
lobby.bannerOf = banners.of
lobby.bannerPicture = banners.picture
lobby.portraitOf = banners.portraitOf
lobby.portraitName = banners.portraitName
lobby.portraitPicture = banners.portrait

-- A room member's size as a card, two to a row of the players' column.
local MEMBER_WIDTH, MEMBER_HEIGHT = 316, 181
local PORTRAIT_SIZE = 44

-- A picture card in the main menu's style: title and a line under it, a
-- word on the right; `onClick` nil for a card that only shows. `shape` is
-- the menu's card class: its small card (the default, of a fixed size), or
-- "bottom-left", a big card's cut corner, as large as `width` and `height`.
local function pictureCard(picture, title, line, right, onClick, enabled, width, height, marks, shape)
	local card
	if cards then
		card = cards.CardButton{
			bottomComponent = cards.makeCardLabelBottomComponent(title, line, right, nil, false),
			onClick = onClick or function() end,
			tooltip = title,
			images = { picture },
			initialImageIndex = 1,
			class = shape or "small-rectangle-card",
			-- A big card is cut to its size, whatever its picture's.
			clipper = shape ~= nil,
			enabled = enabled ~= false,
			extraChildren = marks or {},
		}
	else
		card = builtin.Button{
			meta = { enabled = enabled ~= false },
			content = column({ icon(picture, height - 60), label(title, "font-scale-body"), note(line or "") }),
			onClick = onClick or function() end,
		}
	end
	return builtin.Component{
		meta = { styleSheet = style{ size = { width, height } } },
		layout = builtin.BoxLayout{ children = { card } },
	}
end
lobby.pictureCard = pictureCard

-- Where a member's game is with the room's world: its download, its load,
-- then in the game; before the room starts, whether it is ready.
function lobby.memberStage(member, playing)
	if member.loading == "fetching" then
		return string.format(_("Downloading %d%%"), math.floor(tonumber(member.percent) or 0))
	elseif member.loading == "loading" then
		return _("Loading...")
	elseif playing then
		return member.connected and _("Playing") or nil
	end
	return member.ready and _("Ready") or _("Not ready")
end

-- A room member as a card: their banner, name, and what marks them.
-- `extra`: more of the card's corners (the owner's Remove), as layout
-- children of its picture.
function lobby.memberCard(member, playing, extra)
	local marks = {}
	if member.you then marks[#marks + 1] = _("You") end
	if member.owner then marks[#marks + 1] = _("Owner") end
	if not member.connected then marks[#marks + 1] = _("Away") end
	marks[#marks + 1] = lobby.memberStage(member, playing)
	local differs = lobby.memberDiffers(member)
	if differs then marks[#marks + 1] = differs end
	local ready = member.ready and not playing and (member.loading or "") == "" and builtin.FloatingLayoutChild{
		h = 0.95,
		v = 0.06,
		item = builtin.ImageView{
			meta = { mouseTransparent = true, styleSheet = style{ size = { 22, 22 } } },
			path = ICON.ready,
		},
	} or nil
	-- A member who picked a portrait: it beside their card, which shows
	-- their key's banner (tpf3mp/banners.lua).
	local portrait = lobby.portraitOf(member)
	local corners = {}
	if ready then corners[#corners + 1] = ready end
	for _i, child in ipairs(extra or {}) do corners[#corners + 1] = child end
	local card = pictureCard(lobby.bannerPicture(lobby.bannerOf(member)), member.name,
		table.concat(marks, " · "), member.you and _("You") or nil, nil, true,
		portrait and MEMBER_WIDTH - PORTRAIT_SIZE - 8 or MEMBER_WIDTH, MEMBER_HEIGHT, corners)
	if not portrait then return card end
	return row({ icon(portrait, PORTRAIT_SIZE), gap(8), card })
end

-- A campaign character's portrait as a card of the banner picker: the
-- picture, the character's name, and whether it is yours.
local PORTRAIT_WIDTH, PORTRAIT_HEIGHT = 150, 190
function lobby.portraitCard(id, picked, onClick, enabled)
	return pictureCard(banners.portrait(id), banners.portraitName(id) or id, picked and _("Yours") or " ",
		nil, onClick, enabled, PORTRAIT_WIDTH, PORTRAIT_HEIGHT)
end

-- The pictures of the Host page's play styles. Co-op: the busy harbour of
-- the game's Campaign card, many vessels sharing one port. Competitive: the
-- rusted-out truck left in the desert on the loading screen of the
-- campaign's third mission, a built-in mod of the game; its path is the
-- mod's own (INFERRED to load at the main menu as the campaign's pictures
-- do).
local COOP_PICTURE = "::/gui/menu/images/campaign.tga"
local COMPETITIVE_PICTURE = "urbangames_campaign_mission_03::/gui/mission/m03_loadscreen.tga"
lobby.COOP_PICTURE = COOP_PICTURE
lobby.COMPETITIVE_PICTURE = COMPETITIVE_PICTURE

-- One play style as a card: picked, it says so.
-- The game's own parts the window is built of. (One table: the window's
-- function may hold only so many upvalues.)
local native = {
	cards = content_card,
	tiles = tile_list_react_util,
	mods = mod_manager_react_util,
}

-- One of the game's cards (content_card.tl): its title over `children`.
function native.card(title, children, sheet)
	return builtin.Component{
		meta = { styleSheet = sheet },
		mouseTransparent = true,
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Vertical,
			children = { native.cards.ContentCard{ title = title, extraChildrenPermanent = children } },
		},
	}
end
lobby.card = native.card

-- A line of a card: what on the left, its value on the right, as the game
-- lays out a save's details.
function native.entry(name, value, width)
	return row({ label(name, "font-scale-body"), gui_react_util.makeHorizontalSpacer(), value },
		style{ size = { (width or SIZE.ROOM_RIGHT) - 44, 32 } })
end

-- A tab of the game's tab widget.
function native.tabOf(value, text, item)
	return builtin.TabWidgetChild{
		indicator = builtin.TextView{ meta = { class = "font-scale-tab-widget-indicator" }, text = text },
		item = item,
		value = value,
	}
end

-- A button of the room's footer, all of one size: `class` "primary" for
-- the one that moves the room on, "error-tape" (red) for leaving and
-- taking back being ready.
function native.foot(text, onClick, class, enabled, tooltip)
	return builtin.Button{
		meta = {
			class = class or "secondary",
			enabled = enabled ~= false,
			tooltip = tooltip,
			styleSheet = style{ size = { 200, -1 } },
		},
		content = builtin.TextView{ meta = { class = "font-scale-body" }, text = text },
		onClick = onClick,
	}
end
function native.wide(text, onClick, enabled, tooltip)
	return native.foot(text, onClick, "primary", enabled, tooltip)
end

-- A button on a tile, as the game's tiles have them.
function native.tileButton(text, onClick, enabled, tooltip)
	return builtin.Button{
		meta = { class = "card-and-details, secondary", enabled = enabled ~= false, tooltip = tooltip },
		content = builtin.TextView{ meta = { class = "font-scale-title-4" }, text = text },
		onClick = onClick,
	}
end

-- The picture of the world the room starts from, for a card: the owner's
-- save's own, as the Load Game page shows it (its image, as the card takes
-- one), else its climate's.
function lobby.startPicture(room)
	local start = room.start
	local shot = start and room.you_own and lobby.saveDetails(start.name).shot
	if shot then
		return {
			data_native = shot.data,
			size = api.type.Vec2i.new(shot.width, shot.height),
			scaling = builtin.type.ImageViewScaling.AutoZoom,
		}
	end
	return lobby.fullPicture(start and lobby.bigClimatePicture(start.map) or COOP_PICTURE)
end

-- A climate's picture for a big card: the main menu's own of it (its New
-- Game card's), which fills one; the climate's small icon only for one it
-- has none of.
function lobby.bigClimatePicture(map)
	return CLIMATE_PICTURES[map] or lobby.climatePicture(map)
end

-- A picture by its path, at the size of a save's picture, so that a big
-- card shows any picture as large as a save's.
function lobby.fullPicture(path)
	return {
		path = path,
		size = api.type.Vec2i.new(1920, 1080),
		scaling = builtin.type.ImageViewScaling.AutoZoom,
	}
end

-- A mod's picture on its tile: its Mod Hub logo, as the game's mod selector
-- shows it, else the game's placeholder. Always the game's ModImage, so a
-- mod installed meanwhile changes only what it shows.
function lobby.modImage(id, modio)
	local logo = roommods.logo(id, modio or "")
	return builtin.Component{
		meta = { styleSheet = style{ size = { SIZE.TILE_WIDTH, SIZE.TILE_HEIGHT } } },
		mouseTransparent = true,
		layout = builtin.BoxLayout{ children = {
			native.mods.ModImage{
				context = { backendId = logo and logo.backend or -1 },
				request = logo and logo.request or nil,
				imagePath = MOD_PLACEHOLDER,
			},
		} },
	}
end

-- A Mod Hub download's size, as people read it.
function lobby.sizeText(bytes)
	bytes = tonumber(bytes) or 0
	if bytes <= 0 then return nil end
	if bytes < 1024 * 1024 then return string.format(_("%d KB"), math.max(1, math.ceil(bytes / 1024))) end
	return string.format(_("%.1f MB"), bytes / (1024 * 1024))
end

-- The question before a Mod Hub install, in place of the room's mods: each
-- mod asked about as Mod Hub names it (its logo, title, author and size,
-- for the player to check it is the mod meant, as the owner's number is
-- only a claim), then No or Yes. `asking`: { id, modio, name, details }.
-- `hubPage(a)` shows one on the game's Mod Hub page.
function lobby.installCard(asking, install, cancel, enabled, hubPage)
	local ids, entries = {}, {}
	for _i, a in ipairs(asking) do
		ids[#ids + 1] = a.id
		local d = a.details
		local facts = {}
		if d.author ~= "" then facts[#facts + 1] = string.format(_("by %s"), d.author) end
		facts[#facts + 1] = lobby.sizeText(d.size)
		facts[#facts + 1] = string.format(_("Mod Hub %s"), a.modio)
		local lines = {
			row({ label(d.title ~= "" and d.title or a.id, "font-scale-title-3"), gui_react_util.makeHorizontalSpacer() }),
			gap(6),
			row({ label(table.concat(facts, "  ·  "), "font-scale-body"), gui_react_util.makeHorizontalSpacer() }),
		}
		if d.title ~= "" and a.name ~= "" and d.title ~= a.name then
			lines[#lines + 1] = gap(4)
			lines[#lines + 1] = row({ note(string.format(_("The room calls it %s"), a.name), "warning"),
				gui_react_util.makeHorizontalSpacer() })
		end
		lines[#lines + 1] = gap(10)
		lines[#lines + 1] = row({ button(_("Mod Hub page"), function() hubPage(a) end, nil, enabled,
			_("Its description and pictures on Mod Hub")), gui_react_util.makeHorizontalSpacer() })
		if #entries > 0 then entries[#entries + 1] = gap(12) end
		-- The words beside the logo, from its top left.
		entries[#entries + 1] = row({ lobby.modImage(a.id, a.modio), gap(20),
			column(lines, style{ size = { WIDTH - 64 - SIZE.TILE_WIDTH - 40, SIZE.TILE_HEIGHT } }) })
	end
	local question = #asking == 1
		and string.format(_("Subscribe to %s on Mod Hub?"), asking[1].details.title ~= "" and asking[1].details.title
			or asking[1].id)
		or string.format(_("Subscribe to these %d on Mod Hub?"), #asking)
	return native.card(_("Install from Mod Hub"), { column({
		label(question, "font-scale-title-2"),
		gap(14),
		builtin.ScrollArea{
			meta = { styleSheet = style{ size = { WIDTH - 64, SIZE.MODS_HEIGHT - 110 } } },
			horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
			verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
			content = column(entries),
		},
		gap(12),
		row({
			note(_("Mod Hub subscribes your account to them, then downloads and installs them.")),
			gui_react_util.makeHorizontalSpacer(),
			native.foot(_("Cancel"), cancel),
			gap(8),
			native.foot(_("Subscribe & install"), function() install(ids) end, "primary", enabled),
		}, style{ size = { WIDTH - 64, AUTO } }),
	}) })
end

function lobby.styleCard(competitive, picked, onClick, enabled)
	local title = competitive and _("Competitive") or _("Co-op")
	-- The one picked is marked as a ready player's card is.
	local mark = picked and builtin.FloatingLayoutChild{
		h = 0.95,
		v = 0.06,
		item = builtin.ImageView{
			meta = { mouseTransparent = true, styleSheet = style{ size = { 22, 22 } } },
			path = ICON.ready,
		},
	} or nil
	local card = pictureCard(competitive and COMPETITIVE_PICTURE or COOP_PICTURE, title,
		picked and _("Picked") or " ", nil, onClick, enabled, 300, 170, mark and { mark } or {})
	-- What it means, on the card's tooltip.
	return builtin.Component{
		meta = { tooltip = competitive and _("Each player founds a company of their own in the game.")
			or _("Everyone plays for the room's one company.") },
		layout = builtin.BoxLayout{ children = { card } },
	}
end

-- A big choice of the first page (Join, Host), as a card in the main
-- menu's style.
-- `shape` is a big card's cut corner ("bottom-left", "top-right"), as the
-- main menu's big cards have them.
function lobby.choiceCard(title, line, picture, onClick, enabled, shape)
	local card
	if cards then
		card = cards.CardButton{
			bottomComponent = cards.makeCardLabelBottomComponent(title, line, nil, nil, true),
			onClick = onClick,
			tooltip = line or title,
			images = { picture },
			initialImageIndex = 1,
			class = shape or "small-rectangle-card",
			clipper = shape ~= nil,
			enabled = enabled,
			extraChildren = {},
		}
	else
		card = builtin.Button{
			meta = { enabled = enabled },
			content = column({ icon(picture, CHOICE_HEIGHT - 70), label(title, "font-scale-title-3"), note(line) }),
			onClick = onClick,
		}
	end
	return builtin.Component{
		meta = { styleSheet = style{ size = { CHOICE_WIDTH, CHOICE_HEIGHT } } },
		layout = builtin.BoxLayout{ children = { card } },
	}
end

-- One public room of the list, as a card in the game's own style: the
-- picture of its map, its name, and players, companies and year under it.
-- A server as the room list names it: its name and ping, as "EU · 24 ms".
function lobby.serverLine(name, ping)
	if (tonumber(ping) or 0) > 0 then return string.format(_("%s · %d ms"), name, ping) end
	return name
end

function lobby.roomCard(listed, onClick, enabled)
	local title = listed.name
	local line = string.format(_("%d/%d players · %d %s · %s"), listed.players, listed.max_players,
		listed.companies, listed.companies == 1 and _("company") or _("companies"),
		listed.year > 0 and tostring(listed.year) or _("year unknown"))
	local right = (listed.competitive and _("Competitive") or _("Co-op")) .. " · "
		.. (listed.running and _("Playing") or lobby.climateName(listed.map))
	-- With rooms from the release's several servers: which one, and how far.
	local server = type(listed.server) == "string" and listed.server ~= ""
		and lobby.serverLine(listed.server, listed.ping) or nil
	local sub = string.format(_("%d/%d players · %s"),
		listed.players, listed.max_players, listed.competitive and _("Competitive") or _("Co-op"))
	if server then
		sub = sub .. " · " .. server
		right = right .. " · " .. string.format(_("On %s"), server)
	end
	local lock = listed.has_password and builtin.FloatingLayoutChild{
		h = 0.95,
		v = 0.06,
		item = builtin.ImageView{
			meta = { mouseTransparent = true, styleSheet = style{ size = { 24, 24 } } },
			path = ICON.lock,
		},
	} or nil
	local card
	if cards then
		card = cards.CardButton{
			bottomComponent = cards.makeCardLabelBottomComponent(title, sub, nil, nil, false),
			onClick = onClick,
			tooltip = title .. "\n" .. line .. "\n" .. right .. "\n"
				.. (listed.has_password and _("Has a password") or _("Join this room")),
			images = { lobby.climatePicture(listed.map) },
			initialImageIndex = 1,
			class = "small-rectangle-card",
			enabled = enabled,
			extraChildren = lock and { lock } or {},
		}
	else
		card = builtin.Button{
			meta = { enabled = enabled },
			content = column({
				icon(lobby.climatePicture(listed.map), CARD_HEIGHT - 60),
				label(title, "font-scale-body"),
				note(line),
				note(right),
			}),
			onClick = onClick,
		}
	end
	return builtin.Component{
		meta = { styleSheet = style{ size = { CARD_WIDTH, CARD_HEIGHT } } },
		layout = builtin.BoxLayout{ children = { card } },
	}
end

-- The window's content, rendered inside the Tpf3mpLobbyWindow recipe. ------

-- The room's save and mods, picked on the game's Load Game page
-- (roommods.lua): main_page.tl sends the owner there and back. What they
-- picked waits here until the window is back to send it.
local pendingPick = nil
-- What the Host page held when its owner went to pick on the Load Game
-- page: the game drops the main page, and this window with it, while that
-- page shows, and the window comes back to it as it was.
local keptForPick = nil
function lobby.beginPick(setPage)
	return roommods.begin(setPage, function(choice) pendingPick = choice end)
end
-- Ends a pick under way; whether one was (the window then opens again).
function lobby.endPick()
	return roommods.finish() ~= nil
end
-- The window asked the main menu to come back to it after another page
-- (Mod Hub, to sign in): once.
local reopen = false
function lobby.leaveFor(again)
	reopen = again and true or false
end
function lobby.takeReopen()
	local again = reopen
	reopen = false
	return again
end

-- How the player's game stands to the room's mods, in a line and a tone:
-- nil while the room names none.
function lobby.roomModsLine(state)
	local total = #(state.room_mods or {}) + (tonumber(state.room_mods_more) or 0)
	if total == 0 then return nil end
	local missing, other = tonumber(state.room_mods_missing) or 0, tonumber(state.room_mods_other) or 0
	if missing == 0 and other == 0 then
		return string.format(_("%d mods  ·  you have them all"), total), "success"
	end
	local parts = { string.format(_("%d mods"), total) }
	if missing > 0 then parts[#parts + 1] = string.format(_("%d missing"), missing) end
	if other > 0 then parts[#parts + 1] = string.format(_("%d in another version"), other) end
	return table.concat(parts, "  ·  "), "error"
end

-- How a member's game differs from the room's mods, in words; nil when it
-- does not, or the room does not know.
function lobby.memberDiffers(member)
	local parts = {}
	local missing, changed, extra = tonumber(member.missing) or 0, tonumber(member.changed) or 0,
		tonumber(member.extra) or 0
	if missing > 0 then
		parts[#parts + 1] = missing == 1 and _("1 mod missing") or string.format(_("%d mods missing"), missing)
	end
	if changed > 0 then
		parts[#parts + 1] = changed == 1 and _("1 other version") or string.format(_("%d other versions"), changed)
	end
	if extra > 0 then
		parts[#parts + 1] = extra == 1 and _("1 mod too many") or string.format(_("%d mods too many"), extra)
	end
	if #parts == 0 then return member.content == "differs" and _("Other mods") or nil end
	return table.concat(parts, ", ")
end

-- `focus` is what the card that opened the window is about: "join" puts
-- the invite first.
-- `onPick` sends the owner to the game's Load Game page to pick the room's
-- save and mods; `onModHub` to the game's Mod Hub, to sign in.
-- `commonParams` are the menu's, for its page and top bar.
function lobby.content(onClose, focus, onNewGame, onPick, onModHub, commonParams)
	-- The hook calls this before loading, then lets the menu render one frame.
	-- Polling room state alone races the loader, which suspends menu callbacks.
	resolveutil.__tpf3mp_before_load = onClose
	-- Back from a pick begun on the Host page: that page, as it was. Taken
	-- once, when the window is made.
	local keptRef = react.useRef(false)
	if keptRef:get() == false then
		keptRef:set(keptForPick)
		keptForPick = nil
	end
	local kept = keptRef:get() or {}
	local stateS = react.useState(nil)
	local problemS = react.useState(nil)
	-- An action on its way: { text, polls left, the state it was sent in }.
	local pendingS = react.useState(nil)
	-- Why the hook refused the last action, until the next one.
	local refusedS = react.useState(nil)
	-- A question before kicking or leaving: { kind, id, name }.
	local confirmS = react.useState(nil)
	local name = react.useRef(kept.name or "")
	local roomName = react.useRef(kept.roomName or "")
	local invite = react.useRef("")
	local createPassword = react.useRef(kept.password or "")
	local joinPassword = react.useRef("")
	local joinServerS = react.useState("")
	local chatText = react.useRef("")
	local playersS = react.useState(kept.players or DEFAULT_PLAYERS)
	local rulesS = react.useState(kept.rules)
	local saveS = react.useState(kept.save)
	-- The room page's pick of its start save while the game reads its map
	-- and year: { save, polls }.
	local pickS = react.useState(nil)
	-- The start save this window already told the room the map and year of.
	local describedRef = react.useRef(nil)
	-- What the Host page's pick held besides its save (the mods and their
	-- settings): the room takes them once it is made.
	local hostChoiceRef = react.useRef(kept.choice)
	-- The page shown: "choose" (Join or Host), "join" (the public rooms
	-- and an invite) or "host" (the room's settings); in a room, always the
	-- room's. Your mods show over it while modsS is on.
	local pageS = react.useState(kept.page)
	local modsS = react.useState(false)
	-- In a room, the tab shown: "room", "mods" (the room's) or "own".
	local roomTabS = react.useState(focus == "mods" and "mods" or "room")
	-- The room's mods tab shows only those this player lacks.
	local onlyMissingS = react.useState(false)
	-- The mod whose details a tile's gear asked for: { id, text }.
	local detailS = react.useState(nil)
	-- Installs from Mod Hub, by the room mod's id: { number, step, why,
	-- details, at } with step "looking" (Mod Hub looks it up), "ask" (the
	-- player confirms), "subscribing", "downloading", "done" or "failed";
	-- `at` the poll a lookup or subscription was asked at.
	local installsS = react.useState({})
	-- The polls so far, to time Mod Hub's answers by.
	local pollsRef = react.useRef(0)
	-- The installs as last changed, drawn or not: Mod Hub may answer for
	-- several mods before the window draws again, and each answer changes
	-- what the one before it changed.
	local installsNow = react.useRef(nil)
	local function changeInstalls(change)
		local now = {}
		for id, one in pairs(installsNow:get() or installsS:old()) do now[id] = one end
		-- `change` returns false when it changed nothing.
		if change(now) == false then return end
		installsNow:set(now)
		installsS:set(now)
	end
	-- The banner picker, from the first page.
	local bannerS = react.useState(false)
	-- The Join page's Join with code popup.
	local codeS = react.useState(focus == "code")
	-- The server page, from the first page: whether it shows, the address
	-- typed, and why the launcher refused the last one.
	local serverS = react.useState(false)
	local serverText = react.useRef(nil)
	local serverErrorS = react.useState(nil)
	local publicS = react.useState(kept.public or "private")
	-- The Host page's play style: co-op (false) or competitive.
	local competitiveS = react.useState(kept.competitive or false)
	local joiningS = react.useState(nil)
	local listAtRef = react.useRef(LIST_POLLS)
	-- An explicit click may need a connection first. Keep that intention
	-- separate from the busy indicator; intermediate notices are not success.
	local queued = react.useRef(nil)
	local generate = react.useRef(false)
	local lastSnapshot = react.useRef(nil)
	local copiedS = react.useState(0)
	-- The launcher's latest notice, and how many polls it shows for still.
	local noticeS = react.useState({ text = nil, left = 0 })

	-- What the view shows, in one string: when it changes, an action sent
	-- has been answered.
	local function signature(state)
		if not state then return "" end
		local room = state.room
		local me = you(room)
		return table.concat({
			tostring(state.connection), tostring(room and room.name), tostring(room and room.phase),
			tostring(room and #room.members), tostring(me and me.ready), tostring(state.error),
			tostring(state.notice), tostring(#(state.chat or {})), tostring(state.server_address),
			tostring(room and room.start and room.start.name), tostring(room and room.upload and room.upload.save),
		}, "|")
	end

	-- The page `s` shows: the room's once in one; the first page until
	-- connected; otherwise the one picked, or the one the card that opened
	-- the window is about.
	local function pageOf(s)
		if s.room then return "room" end
		local picked = pageS:old() or (focus == "friend" and "friend" or (focus == "join" and "join" or "choose"))
		if picked == "room" then return "choose" end
		return picked
	end

	-- Poll the hook for the lobby a few times a second: the room and chat
	-- change without anything happening in this window.
	react.onStepTimer(function()
		if copiedS:old() > 0 then copiedS:set(copiedS:old() - 1) end
		local state, why, snapshot = fetchState()
		if state then
			local shown = noticeS:old()
			if state.notice ~= shown.text then
				noticeS:set({ text = state.notice, left = NOTICE_POLLS })
			elseif shown.left > 0 then
				noticeS:set({ text = shown.text, left = shown.left - 1 })
			end
			if problemS:old() ~= nil then problemS:set(nil) end
			local pending = pendingS:old()
			if pending then
				if pending[3] ~= signature(state) or pending[2] <= 1 then
					pendingS:set(nil)
				else
					pendingS:set({ pending[1], pending[2] - 1, pending[3] })
				end
			end
			if snapshot ~= lastSnapshot:get() then
				lastSnapshot:set(snapshot)
				stateS:set(state)
			end
			local nextAction = queued:get()
			if nextAction then
				nextAction.left = nextAction.left - 1
				if state.error and state.error ~= nextAction.error or not state.linked or not state.heard or nextAction.left <= 0 then
					queued:set(nil)
					pendingS:set(nil)
					refusedS:set(state.error or _("Connection timed out. Please try again."))
				elseif state.connection == "connected" and state.name == nextAction.name then
					queued:set(nil)
					local refused = act(nextAction.fields)
					refusedS:set(refused)
					if nextAction.fields.action == "create" then
						generate:set(not refused and nextAction.fields.start_save == "" and onNewGame and { error = state.error } or nil)
					end
					if not refused then pendingS:set({ nextAction.doing, PENDING_POLLS, signature(state) }) end
				end
			end
			if generate:get() and state.room and state.room.you_own then
				generate:set(false)
				if onNewGame then onNewGame() end
			elseif generate:get() and state.error and state.error ~= generate:get().error then
				generate:set(false)
			end
			local room = state.room
			local owning = room and room.you_own and room.phase == "lobby"
			-- A start save picked on the room page goes once the game read its
			-- map and year, or could not in a few polls.
			local pick = pickS:old()
			if pick then
				local details = lobby.saveDetails(pick.save)
				if not details.async or pick.polls >= PICK_POLLS then
					pickS:set(nil)
					if owning then
						refusedS:set(act({ action = "choose_start", save = pick.save, map = details.map or "",
							year = details.year or 0 }))
					end
				else
					pickS:set({ save = pick.save, polls = pick.polls + 1 })
				end
			end
			-- What the owner picked on the game's Load Game page goes to the
			-- room once the window is back.
			if pendingPick then
				local choice = pendingPick
				pendingPick = nil
				if owning then
					local fields
					if choice.mods then
						fields = { action = "choose_room_mods", save = choice.save, map = choice.map or "",
							year = choice.year or 0, mods = choice.mods, params = choice.params }
					else
						fields = { action = "choose_start", save = choice.save, map = "", year = 0 }
					end
					local refused = act(fields)
					refusedS:set(refused)
					if not refused then
						pendingS:set({ _("Taking the save and mods for the room..."), PENDING_POLLS, signature(state) })
					end
				elseif not room and pageOf(state) == "host" then
					-- Hosting: the save goes into the room's settings, its mods
					-- to the room once it is made.
					saveS:set(choice.save)
					hostChoiceRef:set(choice.mods and choice or nil)
				else
					refusedS:set(_("Only the room's owner picks its save and mods, while it is in its lobby."))
				end
			end
			local carried = hostChoiceRef:get()
			if carried and owning and room.start and room.start.name == carried.save then
				hostChoiceRef:set(nil)
				refusedS:set(act({ action = "choose_room_mods", mods = carried.mods, params = carried.params }))
			end
			-- Installs from Mod Hub under way: once the game has the mod, and
			-- it is the room's (the owner's Mod Hub number is only a claim),
			-- the launcher finds the installed mods again.
			-- A lookup or subscription Mod Hub has not answered in HUB_POLLS
			-- fails, and the mod can be installed again.
			local found = false
			local polls = pollsRef:get() + 1
			pollsRef:set(polls)
			changeInstalls(function(updated)
				local changed = false
				for id, install in pairs(updated) do
					local late = polls - (install.at or polls) >= HUB_POLLS
					if install.step == "looking" and late then
						updated[id] = { number = install.number, step = "failed", why = _("Mod Hub did not answer") }
						changed = true
					elseif install.step == "subscribing" or install.step == "downloading" then
						local at = roommods.installState(install.number)
						if at == "none" and install.step == "subscribing" and late then
							updated[id] = { number = install.number, step = "failed", why = _("Mod Hub did not answer") }
							changed = true
						elseif at == "installed" then
							local name = roommods.installedId(install.number)
							if name == id then
								updated[id] = { number = install.number, step = "done" }
								found = true
							else
								updated[id] = { number = install.number, step = "failed",
									why = string.format(_("Mod Hub's mod is %s, not the room's %s"),
										name ~= "" and name or "?", id) }
							end
							changed = true
						elseif at == "failed" then
							updated[id] = { number = install.number, step = "failed", why = _("the download failed") }
							changed = true
						elseif at == "downloading" and install.step ~= "downloading" then
							updated[id] = { number = install.number, step = "downloading", details = install.details }
							changed = true
						end
					end
				end
				return changed
			end)
			if found then refusedS:set(act({ action = "rescan_mods" })) end
			-- The room names its start save without its map and year when the
			-- room was made private: this window tells it what the game read,
			-- once, so every player sees them.
			local start = owning and room.start
			if start and room.upload == nil and start.map == "" and (tonumber(start.year) or 0) == 0
				and describedRef:get() ~= start.name then
				local details = lobby.saveDetails(start.name)
				if not details.async then
					describedRef:set(start.name)
					if details.map ~= "" or details.year > 0 then
						act({ action = "choose_start", save = start.name, map = details.map, year = details.year })
					end
				end
						end
			-- The room list, while it is shown: asked for at once, then
			-- every LIST_POLLS polls (the server allows one a second).
			local browsing = state.linked and state.connection == "connected" and pageOf(state) == "join" and not modsS:old()
			if browsing then
				listAtRef:set(listAtRef:get() + 1)
				if state.rooms == nil and listAtRef:get() >= 3 or listAtRef:get() >= LIST_POLLS then
					listAtRef:set(0)
					act({ action = "list_rooms", page = state.rooms and state.rooms.page or 0 })
				end
			end
		elseif problemS:old() ~= why then
			problemS:set(why)
		end
	end, POLL, false)

	local state = stateS:old()
	local function selectedJoinServer()
		local server = joinServerS:old()
		return server and server ~= "" and server or nil
	end
	local function joinServerChoice()
		local servers = state.servers or {}
		if #servers < 2 then return nil end
		local items = { { "", _("Find automatically across all regions") } }
		for _i, server in ipairs(servers) do
			items[#items + 1] = {
				server.name,
				server.reachable and server.name or (server.name .. _(" (unavailable)")),
			}
		end
		return choice(_("Region (optional)"), joinServerS:old() or "", items, function(value)
			joinServerS:set(value)
		end, _("Leave automatic selected unless your friend chose a region."))
	end

	-- Sends an action, and shows `doing` until the launcher answers.
	local function send(fields, doing)
		confirmS:set(nil)
		local refused = act(fields)
		refusedS:set(refused)
		if fields.action == "create" then
			generate:set(not refused and fields.start_save == "" and onNewGame and { error = stateS:old().error } or nil)
		end
		if not refused and doing then
			pendingS:set({ doing, PENDING_POLLS, signature(stateS:old()) })
		end
		return refused
	end

	local busy = pendingS:old() ~= nil or queued:get() ~= nil
	local function connectedAction(fields, doing)
		if busy then return end
		local current = stateS:old()
		local typed = (name:get() or ""):match("^%s*(.-)%s*$")
		if typed == "" then typed = current.name or "" end
		if typed == "" then refusedS:set(_("Enter your name first.")); return end
		if current.connection == "connected" and current.name == typed then
			send(fields, doing)
		else
			queued:set({ fields = fields, doing = doing, name = typed, error = current.error, left = 75 })
			-- Refused (what was just set shows only from the next draw on):
			-- nothing waits for a connection.
			if send({ action = "connect", name = typed }, _("Connecting...")) then queued:set(nil) end
		end
	end

	-- The page's Back, top left (and the game's Back key): a step back to
	-- where the player came from, out of the window last.
	local function topBack()
		if modsS:old() then
			modsS:set(false)
		elseif codeS:old() then
			codeS:set(false)
		elseif joiningS:old() then
			joiningS:set(nil)
		elseif bannerS:old() then
			bannerS:set(false)
		elseif serverS:old() then
			serverS:set(false)
			serverErrorS:set(nil)
			serverText:set(nil)
		elseif stateS:old() and not stateS:old().room and pageOf(stateS:old()) ~= "choose" then
			queued:set(nil)
			generate:set(false)
			joiningS:set(nil)
			pageS:set("choose")
		else
			onClose()
		end
	end

	-- The page every view shares, as the game's own menu pages are made
	-- (load_game_page.tl): the top bar with Back and "Multiplayer", and the
	-- game's card, with a header (the view's title and the connection) over
	-- a line for what went wrong, what is under way or what just happened,
	-- the room's world when it is coming, the view, and the view's buttons at
	-- the bottom right.
	local function frame(title, status, body, footer)
		local children = {}
		local problem = hideAddress(refusedS:old() or (state and state.error))
		if problem then
			children[#children + 1] = row({ icon(ICON.alert, 18), gap(6), label(problem, "font-scale-body, error") })
		elseif pendingS:old() then
			children[#children + 1] = row({ icon(ICON.loading, 18), gap(6), label(pendingS:old()[1], "font-scale-body, info") })
		elseif state and state.notice and noticeS:old().text == state.notice and noticeS:old().left > 0 then
			children[#children + 1] = note(hideAddress(state.notice))
		else
			children[#children + 1] = gap(18)
		end
		if state and not state.linked then
			children[#children + 1] = label(
				_("This game has no link to the TPF3-MP launcher: close it and start Transport Fever 3 from the launcher."),
				"font-scale-body, error")
		elseif state and not state.heard then
			children[#children + 1] = note(_("Waiting for the launcher..."))
		end
		local world, done = state and worldText(state)
		if world then
			children[#children + 1] = gap(8)
			children[#children + 1] = row({
				label(world, "font-scale-body, info"),
				gap(12),
				builtin.Component{
					meta = { styleSheet = style{ size = { 260, 18 } } },
					layout = builtin.BoxLayout{ children = { builtin.ProgressBar{ value = done } } },
				},
			})
		end
		-- How this game differs from the room's shows on the room's mods
		-- tab, mod by mod; here only what is not about its mods.
		if state and state.differences and not (state.room_mods and #state.room_mods > 0) then
			children[#children + 1] = gap(4)
			children[#children + 1] = label(_("Your game differs from the room's: ") .. state.differences,
				"font-scale-body, warning")
		end
		children[#children + 1] = gap(14)
		children[#children + 1] = body
		children[#children + 1] = gui_react_util.makeVerticalSpacer()
		children[#children + 1] = row(spaced(footer), style{ size = { WIDTH + 2, 44 } })
		local header = {
			gap(12),
			icon(ICON.multiplayer, 28),
			gap(10),
			label(title, "font-scale-title-3"),
			gui_react_util.makeHorizontalSpacer(),
			status,
			gap(12),
		}
		return menu_icon_react_util.makePage(commonParams, _("Multiplayer"), topBack,
			menu_icon_react_util.makeMainOuterCard(true, {
				menu_icon_react_util.makeTabAnalogue(
					{ row(header, style{ size = { WIDTH, 48 } }) },
					{ column(children, style{ size = { WIDTH, HEIGHT }, padding = { 12, 20, 12, 20 } }) }
				),
			}), {})
	end

	-- No answer from the hook yet.
	if not state then
		local why = problemS:old()
		return frame(
			_("Multiplayer"),
			note(_("Waiting for the hook...")),
			label(why and (_("The hook did not answer: ") .. tostring(why)) or "", "font-scale-body, error"),
			{ gui_react_util.makeHorizontalSpacer() }
		)
	end

	local canAct = state.linked and state.heard

	local connected = state.connection == "connected"
	local room = state.room
	local page = pageOf(state)
	local disconnect = function() send({ action = "disconnect" }, _("Disconnecting...")) end

	-- The launcher's run, which every line of its diagnostics carries, with
	-- a Copy: what to quote with a report so the operator reads all of
	-- this run's logs. Nothing while diagnostics are off.
	local function logSession()
		local code = state.log_session
		if code == nil or code == "" then return gap(1) end
		return row({
			note(_("Log session  ")),
			label(code, "font-scale-body, info"),
			gap(6),
			button(copiedS:old() > 0 and _("Copied") or _("Copy"), function()
				local refused = act({ action = "copy", text = code })
				refusedS:set(refused)
				if not refused then copiedS:set(COPIED_POLLS) end
			end, nil, true, _("Copy the log session code, to quote with a report")),
		})
	end

	-- Where the player is, top right: online as whom, on which server.
	local status
	if connected then
		status = row({
			badge(_("Online"), "success"),
			gap(8),
			icon(ICON.player, 18),
			gap(4),
			label(tostring(state.name), "font-scale-body"),
			note("  @ " .. serverName(state)),
		})
	elseif state.connection == "connecting" then
		status = badge(_("Connecting"), "info")
	else
		status = badge(_("Not connected"), "warning")
	end

	local function modsButton()
		local missing = (tonumber(state.room_mods_missing) or 0) + (tonumber(state.room_mods_other) or 0)
		local text
		if missing > 0 then
			text = string.format(_("Mods (%d missing)"), missing)
		else
			local chosen = 0
			for _i, m in ipairs(state.mods or {}) do
				if m.chosen and m.choosable then chosen = chosen + 1 end
			end
			text = string.format(_("Your mods (%d chosen)"), chosen)
		end
		return native.foot(text, function() modsS:set(true) end, nil,
			#(state.mods or {}) > 0 or #(state.room_mods or {}) > 0)
	end

	-- Installs the room's mods in `list` from Mod Hub: each looked up in
	-- this player's Mod Hub first, then confirmed (the owner's number is
	-- only a claim of where to get it).
	local function lookUp(list)
		local at = pollsRef:get()
		changeInstalls(function(updated)
			for _i, m in ipairs(list) do updated[m.id] = { number = m.modio, step = "looking", at = at } end
		end)
		for _i, m in ipairs(list) do
			roommods.lookUp(m.modio, function(details, why)
				changeInstalls(function(now)
					-- An answer after the lookup gave up, or for another one,
					-- changes nothing.
					local one = now[m.id]
					if not (one and one.step == "looking" and one.number == m.modio) then return false end
					if details then
						now[m.id] = { number = m.modio, step = "ask", details = details }
					else
						now[m.id] = { number = m.modio, step = "failed", why = why }
					end
				end)
			end)
		end
	end
	local function install(ids)
		local asked = {}
		local at = pollsRef:get()
		changeInstalls(function(updated)
			for _i, id in ipairs(ids) do
				local one = updated[id]
				if one and one.step == "ask" then
					updated[id] = { number = one.number, step = "subscribing", details = one.details, at = at }
					asked[#asked + 1] = { id = id, number = one.number }
				end
			end
		end)
		for _i, one in ipairs(asked) do
			local id = one.id
			roommods.install(one.number, function(why)
				if why then
					changeInstalls(function(now)
						local was = now[id]
						if not (was and was.step == "subscribing" and was.number == one.number) then return false end
						now[id] = { number = one.number, step = "failed", why = why }
					end)
				end
			end)
		end
	end
	-- The game's own Mod Hub page of one of the room's mods, to see it and
	-- subscribe there; once closed, an install it began is followed as one
	-- begun here. False when the game could not show it.
	local function hubPage(m)
		return roommods.showDetails(commonParams, m.modio, m.name ~= "" and m.name or m.id, function()
			if roommods.installState(m.modio) ~= "none" then
				changeInstalls(function(now)
					local one = now[m.id]
					if one and one.step ~= "ask" and one.step ~= "failed" then return false end
					now[m.id] = { number = m.modio, step = "subscribing", at = pollsRef:get() }
				end)
			end
		end)
	end
	-- The room's save and mods, on the game's Load Game page; said here
	-- when the game's page is not one this mod can pick on.
	local function pickWorld()
		local hosting = stateS:old() and pageOf(stateS:old()) == "host" and {
			page = "host",
			name = name:get(),
			roomName = roomName:get(),
			password = createPassword:get(),
			players = playersS:old(),
			rules = rulesS:old(),
			public = publicS:old(),
			competitive = competitiveS:old(),
			-- A save picked before, with its mods: kept should this pick
			-- end without one.
			save = saveS:old(),
			choice = hostChoiceRef:get(),
		} or nil
		onPick()
		if roommods.isPicking() then
			keptForPick = hosting
		else
			refusedS:set(_("The save can't be picked on this game's Load Game page: see the game's log"))
		end
	end
	-- No: the mods asked about go back to their Install.
	local function cancelAsking()
		changeInstalls(function(now)
			for id, one in pairs(now) do
				if one.step == "ask" then now[id] = nil end
			end
		end)
	end

	-- The game's tiles for `elements`, scrolled as its own pages scroll them.
	local function tiles(elements, height, empty)
		if #elements == 0 then return note(empty) end
		return builtin.ScrollArea{
			meta = { class = "tile-list-scroll", styleSheet = style{ size = { WIDTH - 40, height } } },
			horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
			verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
			content = builtin.Component{
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Vertical,
					children = { native.tiles.TileList{ elements = elements, numRows = -1, numCols = SIZE.TILE_COLUMNS } },
				},
			},
		}
	end
	local calloutRef = commonParams and commonParams.calloutContainerRef

	-- The room's mods as the game's mod selector shows mods: a tile each,
	-- with whether this player has it, and Mod Hub's to install.
	local function roomModsPanel()
		local playing = room and room.phase == "playing"
		local owner = room and room.you_own
		local hub = roommods.hubState()
		local installs = installsS:old()
		local elements, missing, asking, looking = {}, {}, {}, 0
		for _i, m in ipairs(state.room_mods or {}) do
			local one = installs[m.id]
			local versions = m.version ~= "" and ("v" .. m.version) or _("no version")
			if m.yours ~= "" and m.yours ~= m.version then
				versions = string.format(_("room v%s, yours v%s"), m.version, m.yours)
			end
			local details = table.concat({ m.id, versions, m.source ~= "" and m.source or "?" }, "  ·  ")
			local icons = {}
			if m.have == "yes" then
				icons[#icons + 1] = { path = ICON.ready, label = _("Installed"), tooltip = details, secondary = false }
			elseif one and one.step == "done" then
				-- The game has it, the launcher does not find it (yet): not
				-- the room's until it does.
				icons[#icons + 1] = { path = ICON.alert, label = _("Installed, not found yet"), tooltip = details,
					secondary = false, critical = false }
			elseif m.have == "other_version" then
				icons[#icons + 1] = { path = ICON.alert, label = _("Another version"), tooltip = details,
					secondary = false, critical = false }
			else
				icons[#icons + 1] = { path = ICON.alert, label = _("Missing"), tooltip = details, secondary = false,
					critical = false }
			end
			icons[#icons + 1] = {
				path = m.modio ~= "" and "::/gui/menu/icons/mod_management/source_modio.tga" or ICON.save,
				label = m.source == "mod.io" and "Mod Hub" or (m.source ~= "" and m.source or "?"),
				tooltip = details,
				secondary = true,
			}
			local buttons = {}
			if m.have ~= "yes" and not owner then
				if m.modio == "" then
					icons[1].tooltip = details .. "\n" .. _("Not on Mod Hub: ask the owner where to get it")
				elseif one and one.step == "looking" then
					looking = looking + 1
					buttons[1] = native.tileButton(_("Looking up..."), function() end, false)
				elseif one and one.step == "ask" then
					asking[#asking + 1] = { id = m.id, modio = m.modio, name = m.name, details = one.details }
				elseif one and (one.step == "subscribing" or one.step == "downloading") then
					buttons[1] = native.tileButton(_("Installing..."), function() end, false)
				elseif one and one.step == "done" then
					buttons[1] = native.tileButton(_("Look again"), function()
						refusedS:set(act({ action = "rescan_mods" }))
					end, canAct, _("Find the installed mods again"))
				else
					if one and one.step == "failed" then
						icons[1].tooltip = details .. "\n" .. tostring(one.why)
						icons[1].label = _("Install failed")
					end
					missing[#missing + 1] = { id = m.id, modio = m.modio }
					-- The game's Mod Hub page of it, to see and subscribe; asked
					-- here when the game cannot show it.
					buttons[1] = native.tileButton(_("Install"), function()
						if not hubPage(m) then lookUp({ { id = m.id, modio = m.modio } }) end
					end, canAct and hub == "ok" and not playing, _("Its Mod Hub page, to see it and subscribe"))
				end
			end
			local shown = not onlyMissingS:old() or m.have ~= "yes"
			if shown then
				elements[#elements + 1] = native.tiles.TileElement{
					meta = { localKey = "room-mod-" .. m.id },
					title = m.name ~= "" and m.name or m.id,
					tileTooltip = details,
					infoIcons = icons,
					createImage = function() return lobby.modImage(m.id, m.modio) end,
					-- A new list each time: the game's tile adds its Details
					-- button to the list it is given.
					createButtons = function()
						local fresh = {}
						for i, b in ipairs(buttons) do fresh[i] = b end
						return fresh
					end,
					onClickDetails = function()
						detailS:set({ id = m.id, text = (m.name ~= "" and m.name or m.id) .. ":  " .. details })
					end,
					calloutContainerRef = calloutRef,
				}
			end
		end

		-- Asked to install, once Mod Hub answered for all: the mods as Mod
		-- Hub names them, in place of the tiles, until the player says yes
		-- or no.
		if #asking > 0 and looking == 0 then return lobby.installCard(asking, install, cancelAsking, canAct, hubPage) end

		local line, tone = lobby.roomModsLine(state)
		local top = {}
		if line then top[#top + 1] = badge(line, tone) end
		top[#top + 1] = gap(12)
		local detail = detailS:old()
		top[#top + 1] = detail and note(detail.text) or gap(1)
		top[#top + 1] = gui_react_util.makeHorizontalSpacer()
		top[#top + 1] = button(onlyMissingS:old() and _("Show all") or _("Only missing"),
			function() onlyMissingS:set(not onlyMissingS:old()) end, nil, true)
		top[#top + 1] = gap(8)
		if #missing > 0 and hub == "ok" then
			top[#top + 1] = primary(string.format(_("Install all missing (%d)"), #missing), function()
				lookUp(missing)
			end, canAct and not playing, _("Look them up on Mod Hub, then install them"))
		elseif #missing > 0 and hub == "signed_out" then
			top[#top + 1] = note(_("Sign in to Mod Hub to install them"), "warning")
			top[#top + 1] = gap(8)
			top[#top + 1] = button(_("Mod Hub"), onModHub, nil, onModHub ~= nil)
		elseif #missing > 0 then
			top[#top + 1] = note(_("Mod Hub is not available"), "warning")
		end
		local children = { row(top, style{ size = { WIDTH - 40, 40 } }), gap(8) }
		children[#children + 1] = tiles(elements, SIZE.MODS_HEIGHT,
			onlyMissingS:old() and _("You have all of the room's mods.") or _("The room names no mods yet."))
		if (state.room_mods_more or 0) > 0 then
			children[#children + 1] = note(string.format(_("and %d more"), state.room_mods_more))
		end
		return column(children)
	end

	-- This player's own mods, the ones only they play with: the game's tiles
	-- with its Activate button.
	local function ownModsPanel(height)
		local playing = room and room.phase == "playing"
		local elements = {}
		for _i, m in ipairs(state.mods or {}) do
			if m.choosable then
				elements[#elements + 1] = native.tiles.TileElement{
					meta = { localKey = "own-mod-" .. m.id },
					title = m.name,
					tileTooltip = m.reason,
					infoIcons = { {
						path = m.class == "carried" and ICON.multiplayer or ICON.player,
						label = m.class == "carried" and _("carried by the room") or _("only you see it"),
						tooltip = m.reason,
						secondary = false,
					} },
					createImage = function() return lobby.modImage(m.id, "") end,
					createButtons = function()
						return { native.mods.ModActivateButton{
							active = m.chosen,
							missing = false,
							onValueChange = function(value)
								if canAct and not playing then
									send({ action = "choose_mod", id = m.id, chosen = value }, nil)
								end
							end,
						} }
					end,
					onClickDetails = function()
						detailS:set({ id = m.id, text = m.name .. " (" .. m.id .. "):  " .. tostring(m.reason) })
					end,
					calloutContainerRef = calloutRef,
				}
			end
		end
		local detail = detailS:old()
		return column({
			row({
				note(playing and _("The room's game has started: your choice holds for its next world.")
					or (detail and detail.text) or ""),
			}, style{ size = { WIDTH - 40, 40 } }),
			gap(8),
			tiles(elements, height or SIZE.MODS_HEIGHT, _("No mods of your own besides the room's.")),
		})
	end

	-- Your mods, outside a room: over whichever page opened it, with Back
	-- to it.
	if modsS:old() and page ~= "choose" and not room then
		-- A running room that refused this game for its mods told them:
		-- they show, to install what is missing, before joining again.
		if #(state.room_mods or {}) > 0 then
			local missing = (tonumber(state.room_mods_missing) or 0) + (tonumber(state.room_mods_other) or 0)
			local count = #state.room_mods + (tonumber(state.room_mods_more) or 0)
			return frame(_("Mods"), status, builtin.TabWidget{
				orientation = builtin.type.TabOrientation.North,
				deselectAllowed = false,
				showIndicators = true,
				value = roomTabS:old() == "own" and "own" or "mods",
				tabs = {
					native.tabOf("mods", missing > 0 and string.format(_("The room's mods (%d) · %d missing"), count,
						missing) or string.format(_("The room's mods (%d)"), count), roomModsPanel()),
					native.tabOf("own", _("Only for you"), ownModsPanel()),
				},
				onValueChange = function(value) roomTabS:set(value) end,
			}, { gui_react_util.makeHorizontalSpacer() })
		end
		return frame(_("Your mods"), status, ownModsPanel(HEIGHT - 150), { gui_react_util.makeHorizontalSpacer() })
	end

	-- The small buttons of a page's footer on its left, then what is on its
	-- right, as the room's footer lays them out.
	local function footerOf(left, right)
		local smalls = {}
		for _i, item in ipairs(left) do
			if #smalls > 0 then smalls[#smalls + 1] = gap(8) end
			smalls[#smalls + 1] = item
		end
		local footer = { row(smalls), gui_react_util.makeHorizontalSpacer() }
		for _i, item in ipairs(right or {}) do footer[#footer + 1] = item end
		return footer
	end

	-- Your banner, from the first page: the picture the others see on your
	-- card in a room. A click picks one; Default goes back to the one your
	-- key gives.
	if bannerS:old() and page == "choose" then
		local rows, cellsRow = {}, {}
		for _i, banner in ipairs(BANNERS) do
			local picked = state.banner == banner[1]
			if #cellsRow > 0 then cellsRow[#cellsRow + 1] = gap(10) end
			cellsRow[#cellsRow + 1] = pictureCard(banner[2], picked and _("Yours") or " ", nil, nil, function()
				send({ action = "set_banner", banner = banner[1] }, nil)
			end, canAct, MEMBER_WIDTH, MEMBER_HEIGHT)
			if #cellsRow >= 7 then
				rows[#rows + 1] = row(cellsRow)
				rows[#rows + 1] = gap(10)
				cellsRow = {}
			end
		end
		if #cellsRow > 0 then rows[#rows + 1] = row(cellsRow) end
		-- The campaign's characters this game has (the launcher takes their
		-- portraits from the game): one shows beside your name instead.
		local portraits = {}
		for _i, id in ipairs(state.portraits or {}) do
			if banners.portrait(id) then portraits[#portraits + 1] = id end
		end
		local cardsShown = {
			native.card(_("Banners"), {
				builtin.ScrollArea{
					meta = { styleSheet = style{ size = { WIDTH - 64, #portraits > 0 and 300 or HEIGHT - 170 } } },
					horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
					verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
					content = column(rows),
				},
			}),
		}
		if #portraits > 0 then
			local prows = {}
			cellsRow = {}
			for _i, id in ipairs(portraits) do
				if #cellsRow > 0 then cellsRow[#cellsRow + 1] = gap(10) end
				cellsRow[#cellsRow + 1] = lobby.portraitCard(id, state.banner == id, function()
					send({ action = "set_banner", banner = id }, nil)
				end, canAct)
				if #cellsRow >= 17 then
					prows[#prows + 1] = row(cellsRow)
					prows[#prows + 1] = gap(10)
					cellsRow = {}
				end
			end
			if #cellsRow > 0 then prows[#prows + 1] = row(cellsRow) end
			cardsShown[#cardsShown + 1] = gap(8)
			cardsShown[#cardsShown + 1] = native.card(_("Characters"), {
				builtin.ScrollArea{
					meta = { styleSheet = style{ size = { WIDTH - 64, HEIGHT - 520 } } },
					horizontalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
					verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
					content = column(prows),
				},
			})
		end
		return frame(_("Your banner"), status, column(cardsShown), footerOf({
			native.foot(_("Default"), function() send({ action = "set_banner", banner = "" }, nil) end, nil,
				canAct and state.banner ~= nil and state.banner ~= "", _("The banner your key gives")),
		}))
	end

	-- The server this launcher plays on, from the first page: shown,
	-- changed, or put back to the launcher's own. Not while in a room.
	if serverS:old() and page == "choose" then
		local onDefault = state.server_address == state.server_default
		if serverText:get() == nil then serverText:set(state.server_address or "") end
		local usable = canAct and not room and not busy
		local function use(address)
			local refused = act({ action = "set_server", server = address })
			serverErrorS:set(refused)
			refusedS:set(nil)
			if not refused then
				serverText:set(address ~= "" and address or nil)
				pendingS:set({ _("Changing the server..."), PENDING_POLLS, signature(stateS:old()) })
			end
		end
		-- The launcher's own errors show at the top, as on every page.
		local problem = serverErrorS:old()
		local width = SIZE.ROOM_RIGHT
		local children = {
			native.entry(_("Playing on"), label(serverName(state) .. (onDefault and _(" (default)") or "")), width),
			gap(8),
			field(_("Server address (host:port)"), serverText, state.server_default ~= "" and state.server_default or "host:port",
				{ maxLength = 128, onEnter = function(value) if usable then use(value) end end }),
			problem and label(problem, "font-scale-body, error") or gap(1),
			gap(8),
			note(_("Changing the server disconnects you and connects to the new one. Invites only join rooms on your own server.")),
		}
		local right = {}
		if not onDefault then
			right[#right + 1] = native.foot(_("Reset to default"), function() use("") end, nil, usable)
			right[#right + 1] = gap(8)
		end
		right[#right + 1] = native.foot(_("Use this server"), function() use(serverText:get() or "") end, "primary", usable)
		return frame(_("Server"), status, row({
			native.card(_("Server"), { column(children, style{ size = { width - 24, 190 } }) }),
			gap(16),
			native.card(_("Diagnostics"), { column({
				logSession(),
			}, style{ size = { SIZE.ROOM_LEFT - 24, 190 } }) }),
		}), footerOf({}, right))
	end

	-- A friend's invite is a complete journey, including first connection.
	if page == "friend" then
		local function joinFriend()
			local code = (invite:get() or ""):gsub("%s", ""):upper()
			if not code:match("^[A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9]$") then
				refusedS:set(_("Enter the six-character invite code your friend sent you.")); return
			end
			connectedAction({
				action = "join", invite = code, server = selectedJoinServer(),
				password = joinPassword:get() or "",
			}, _("Joining the room..."))
		end
		local fields = {
			field(_("Your name"), name, state.name ~= "" and state.name or _("Your name"), { maxLength = 32 }),
			field(_("Invite code"), invite, "K7QM2X", { maxLength = 16 }),
			field(_("Password (optional)"), joinPassword, "", { password = true, maxLength = 64 }),
		}
		local region = joinServerChoice()
		if region then fields[#fields + 1] = region end
		return frame(_("Join a friend"), status, row({
			native.card(_("Join a friend"), { column(fields, style{ size = { SIZE.ROOM_RIGHT - 24, AUTO } }) }),
		}), footerOf({}, { native.foot(_("Join room"), joinFriend, "primary", canAct and not busy) }))
	end

	-- The first page: identity, then a clear choice of hosting or discovery,
	-- as two of the main menu's big cards.
	if page == "choose" then
		local connecting = state.connection == "connecting"
		local function connect()
			local typed = name:get()
			if typed == nil or typed:match("^%s*$") then typed = state.name end
			send({ action = "connect", name = typed }, _("Connecting to ") .. serverName(state) .. "...")
		end
		local top
		if connected then
			top = gap(1)
		else
			top = row({
				label(_("Your name"), "font-scale-body"),
				gap(8),
				input(name, state.name ~= "" and state.name or _("Your name"), 260,
					{ maxLength = 32, acceptOnFocusLoss = true }),
				gap(10),
				native.foot(connecting and _("Connecting...") or string.format(_("Connect to %s"), serverName(state)),
					connect, "primary", canAct and not connecting and not busy),
			}, style{ size = { WIDTH - 40, 42 } })
		end
		local lefts = {}
		if connected then lefts[#lefts + 1] = native.foot(_("Disconnect"), disconnect, "error-tape", canAct) end
		lefts[#lefts + 1] = native.foot(_("Server..."), function() serverS:set(true) end, nil, canAct)
		lefts[#lefts + 1] = native.foot(_("Your banner"), function() bannerS:set(true) end, nil, canAct)
		return frame(
			_("Build something together"),
			status,
			column({
				top,
				gap(24),
				row({
					gui_react_util.makeHorizontalSpacer(),
					lobby.choiceCard(_("Join a room"), nil,
						"::/gui/menu/images/m05_ingame.tga", function() pageS:set("join") end, canAct and not busy,
						"bottom-left"),
					gap(24),
					lobby.choiceCard(_("Host a room"), nil,
						"::/gui/menu/images/m02_ingame.tga", function() pageS:set("host") end, canAct and not busy,
						"top-right"),
					gui_react_util.makeHorizontalSpacer(),
				}, style{ size = { WIDTH - 40, CHOICE_HEIGHT } }),
			}, style{ size = { WIDTH - 40, CHOICE_HEIGHT + 70 } }),
			footerOf(lefts, { logSession() })
		)
	end

	local function joinBy(code, password, server)
		code = (code or ""):gsub("%s", ""):upper()
		if code == "" then
			refusedS:set(_("Type the invite code a friend sent you."))
			return
		end
		joiningS:set(nil)
		send({ action = "join", invite = code, server = server, password = password or "" }, _("Joining the room..."))
	end

	-- Join: the public rooms, as cards, and an invite.
	if page == "join" then
		if not connected then
			return frame(_("Discover public rooms"), status, row({
				native.card(_("Your name"), { column({
					field(_("Your name"), name, state.name ~= "" and state.name or _("Your name"), { maxLength = 32 }),
				}, style{ size = { SIZE.ROOM_RIGHT - 24, AUTO } }) }),
			}), footerOf({ modsButton() }, {
				native.foot(_("Browse rooms"), function()
					connectedAction({ action = "list_rooms", page = 0 }, _("Finding rooms..."))
				end, "primary", canAct and not busy),
			}))
		end
		local list = state.rooms
		local found = list and list.list or {}
		local at = list and list.page or 0
		local function askPage(n)
			listAtRef:set(0)
			send({ action = "list_rooms", page = n }, nil)
		end
		local shown = {}
		for _i, listed in ipairs(found) do
			shown[#shown + 1] = lobby.roomCard(listed, function()
				local server = type(listed.server) == "string" and listed.server ~= "" and listed.server or nil
				if listed.has_password then
					joiningS:set({ invite = listed.invite, name = listed.name, server = server })
				else
					joinBy(listed.invite, "", server)
				end
			end, canAct and not busy)
		end
		local rows = {}
		for first = 1, #shown, CARDS_PER_ROW do
			local cells = {}
			for i = first, math.min(first + CARDS_PER_ROW - 1, #shown) do
				if i > first then cells[#cells + 1] = gap(12) end
				cells[#cells + 1] = shown[i]
			end
			rows[#rows + 1] = row(cells)
			rows[#rows + 1] = gap(12)
		end
		if #rows == 0 then
			rows[1] = note(list and _("No public rooms right now. Host one, and make it public.")
				or _("Asking the server for its rooms..."))
		end
		local children = {
			row({
				gui_react_util.makeHorizontalSpacer(),
				button(_("Previous"), function() askPage(at - 1) end, nil, canAct and at > 0),
				gap(6),
				button(_("Next"), function() askPage(at + 1) end, nil, canAct and list ~= nil and list.more),
				gap(6),
				button(_("Refresh"), function() askPage(at) end, nil, canAct),
			}, style{ size = { WIDTH - 64, 36 } }),
			gap(6),
			builtin.ScrollArea{
				meta = { styleSheet = style{ size = { WIDTH - 64, HEIGHT - 250 } } },
				horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
				verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
				content = column(rows),
			},
		}
		local joining = joiningS:old()
		if joining then
			children[#children + 1] = gap(8)
			children[#children + 1] = row({
				label(string.format(_("%s has a password:"), joining.name), "font-scale-body"),
				gap(8),
				input(joinPassword, _("Password"), 220, {
					password = true, maxLength = 64,
					onEnter = function(value) joinBy(joining.invite, value, joining.server) end,
				}),
				gap(8),
				primary(_("Join"), function() joinBy(joining.invite, joinPassword:get(), joining.server) end, canAct and not busy),
				gap(6),
				button(_("Cancel"), function() joiningS:set(nil) end),
			})
		end
		local body = native.card(string.format(_("Public rooms on %s"), lobby.listedServers(list) or serverName(state)), { column(children) })
		local right = { native.foot(_("Join with code"), function()
			joiningS:set(nil)
			codeS:set(true)
		end, "primary", canAct) }
		-- Join with code: in place of the room list, laid out as the Host
		-- page: the picture of joining on the left, the invite (large, as
		-- the room's page shows it), its password and where it joins on the
		-- right; Join or Cancel below.
		if codeS:old() then
			local function join()
				codeS:set(false)
				joinBy(invite:get(), joinPassword:get(), selectedJoinServer())
			end
			local region = joinServerChoice()
			local tall = SIZE.PREVIEW_HEIGHT + SIZE.PLAYERS_HEIGHT + (region and 154 or 46)
			local picture = pictureCard(lobby.fullPicture("::/gui/menu/images/m05_ingame.tga"), _("Join with code"),
				_("Six letters and digits, on the room's page of whoever hosts it"), nil, nil, true,
				SIZE.ROOM_LEFT - 24, tall, nil, "bottom-left")
			local code = builtin.TextInputField{
				meta = { class = "font-scale-title-2", styleSheet = style{ size = { SIZE.ROOM_RIGHT - 44, 64 } } },
				value = invite:get(),
				placeholderText = "K7QM2X",
				maxLength = 128,
				acceptOnFocusLoss = true,
				resetValueOnCancel = false,
				onValueChange = function(value) invite:set(value) end,
				onTyping = function(value) invite:set(value) end,
			}
			local inviteFields = {
				column({
					note(_("Invite code")),
					gap(4),
					code,
				}, style{ size = { SIZE.ROOM_RIGHT - 44, 100 } }),
				gap(12),
				note(_("Password (if the room has one)")),
				gap(4),
				input(joinPassword, "", SIZE.ROOM_RIGHT - 44, { password = true, maxLength = 64 }),
			}
			if region then
				inviteFields[#inviteFields + 1] = gap(12)
				inviteFields[#inviteFields + 1] = region
			end
			inviteFields[#inviteFields + 1] = gap(12)
			inviteFields[#inviteFields + 1] = native.entry(_("Server"), label(serverName(state), "font-scale-body"))
			inviteFields[#inviteFields + 1] = native.entry(_("You join as"), label(state.name ~= "" and state.name or "?", "font-scale-body"))
			inviteFields[#inviteFields + 1] = gui_react_util.makeVerticalSpacer()
			body = row({
				column({ native.card(_("Room"), { picture }) }, style{ size = { SIZE.ROOM_LEFT, AUTO } }),
				gap(16),
				column({
					-- From the top: the code, its password, then where it joins;
					-- what is left below.
					native.card(_("Invite"), { column(inviteFields, style{ size = { SIZE.ROOM_RIGHT - 24, tall } }) }),
				}, style{ size = { SIZE.ROOM_RIGHT, AUTO } }),
			})
			right = {
				native.foot(_("Cancel"), function() codeS:set(false) end),
				gap(8),
				native.foot(_("Join"), join, "primary", canAct and not busy),
			}
		end
		return frame(_("Join a room"), status, body, footerOf({ modsButton() }, right))
	end

	-- Host: the room's world, as the room shows it, and its settings, and
	-- Create.
	if page == "host" then
		local rules = state.rules or {}
		local rulesItems = {}
		for i, offered in ipairs(rules) do
			rulesItems[#rulesItems + 1] = { offered.name, i == 1 and (offered.name .. _(" (default)")) or offered.name }
		end
		local pickedRules = rulesS:old()
		local explainRules
		for _i, offered in ipairs(rules) do
			if offered.name == (pickedRules or (rules[1] and rules[1].name)) then explainRules = offered.description end
		end
		local saves = state.saves or {}
		local pickedSave = saveS:old()
		if pickedSave == nil then
			pickedSave = ""
			for _i, save in ipairs(saves) do
				if save == state.start_save then pickedSave = save end
			end
		end
		local playersItems = {}
		for n = MIN_PLAYERS, MAX_PLAYERS do
			playersItems[#playersItems + 1] = { n, string.format(_("%d players"), n) }
		end
		local public = publicS:old() == "public"
		local details = pickedSave ~= "" and lobby.saveDetails(pickedSave) or nil
		local function create()
			local named = roomName:get()
			if named == nil or named:match("^%s*$") then
				local hostName = (name:get() or ""):match("^%s*(.-)%s*$")
				named = string.format(_("%s's room"), hostName ~= "" and hostName or state.name)
			end
			local fields = {
				action = "create",
				room = named,
				password = createPassword:get() or "",
				max_players = playersS:old(),
				rules = pickedRules or "",
				start_save = pickedSave,
				public = public,
				competitive = competitiveS:old() == true,
			}
			if public then
				fields.map = details and details.map or ""
				fields.year = details and details.year or 0
			end
			connectedAction(fields, _("Creating the room..."))
		end
		local where = public
			and (details and details.map ~= ""
				and string.format(_("Listed for everyone on %s: %s, %s."), serverName(state),
					lobby.climateName(details.map), details.year > 0 and tostring(details.year) or _("year unknown"))
				or string.format(_("Listed for everyone on %s."), serverName(state)))
			or nil
		-- The world, as the room's page shows it: a click picks the save,
		-- its mods and their settings on the game's Load Game page.
		local worldTitle, worldPicture
		if pickedSave ~= "" then
			worldTitle = lobby.startLine({ name = pickedSave, map = details and details.map or "",
				year = details and details.year or 0, arrived = true })
			worldPicture = details and details.shot and {
				data_native = details.shot.data,
				size = api.type.Vec2i.new(details.shot.width, details.shot.height),
				scaling = builtin.type.ImageViewScaling.AutoZoom,
			} or lobby.fullPicture(details and details.map ~= "" and lobby.bigClimatePicture(details.map) or COOP_PICTURE)
		else
			worldTitle = _("New world")
			worldPicture = lobby.fullPicture("::/gui/menu/images/temperate_ingame.tga")
		end
		local world = pictureCard(worldPicture, worldTitle,
			pickedSave ~= "" and _("Click to choose the save and mods")
				or _("Choose your map and settings on the next screen."), nil,
			onPick and pickWorld or nil, canAct and not busy, SIZE.ROOM_LEFT - 24,
			SIZE.PREVIEW_HEIGHT, nil, "bottom-left")
		local left = column({
			native.card(_("World"), { world }),
			gap(8),
			native.card(_("How you play"), { column({
				row({
					lobby.styleCard(false, competitiveS:old() ~= true, function() competitiveS:set(false) end, canAct),
					gap(12),
					lobby.styleCard(true, competitiveS:old() == true, function() competitiveS:set(true) end, canAct),
				}),
			}, style{ size = { SIZE.ROOM_LEFT - 24, SIZE.PLAYERS_HEIGHT } }) }),
		}, style{ size = { SIZE.ROOM_LEFT, AUTO } })
		local right = column({
			native.card(_("Room"), { column({
				not connected and field(_("Your name"), name, state.name ~= "" and state.name or _("Your name"), { maxLength = 32 }) or gap(1),
				field(_("Room name"), roomName, string.format(_("%s's room"), state.name), { maxLength = 48 }),
				choice(_("Players"), playersS:old(), playersItems, function(value) playersS:set(value) end),
				choice(_("Who can find it"), public and "public" or "private", {
					{ "private", _("Private: invite only") },
					{ "public", _("Public: in the room list") },
				}, function(value) publicS:set(value) end, where),
				#rulesItems > 1 and choice(_("Rules"), pickedRules or rulesItems[1][1], rulesItems,
					function(value) rulesS:set(value) end, explainRules) or gap(1),
				field(_("Password (optional)"), createPassword, "", { password = true, maxLength = 64 }),
			}, style{ size = { SIZE.ROOM_RIGHT - 24, SIZE.PREVIEW_HEIGHT + SIZE.PLAYERS_HEIGHT + 46 } }) }),
		}, style{ size = { SIZE.ROOM_RIGHT, AUTO } })
		return frame(_("Host a room"), status, row({ left, gap(16), right }), footerOf({ modsButton() }, {
			native.foot(_("Create room"), create, "primary", canAct and not busy),
		}))
	end

	-- In a room: players on the left, chat on the right.
	local me = you(room)
	local playing = room.phase == "playing"
	local confirm = confirmS:old()
	-- The players as cards of their banners, two to a row; for the owner,
	-- a Remove under each other player's, asked first.
	local memberRows = {}
	local cells = {}
	local function flush()
		if #cells > 0 then
			memberRows[#memberRows + 1] = row(cells)
			memberRows[#memberRows + 1] = gap(10)
			cells = {}
		end
	end
	for _i, member in ipairs(room.members) do
		-- For the owner, a Remove in each other player's card's top left
		-- corner (its check mark is in the top right), asked first: every
		-- card stays as high as the others.
		local extra = {}
		if room.you_own and not member.you then
			local item
			if confirm and confirm.kind == "kick" and confirm.id == member.id then
				item = row({
					button(_("Remove"), function()
						send({ action = "kick", player = member.id }, string.format(_("Removing %s..."), member.name))
					end, "error-tape", canAct),
					gap(4),
					button(_("Keep"), function() confirmS:set(nil) end),
				})
			else
				item = button_react_util.makeIconButton(nil, ICON.kick, function()
					confirmS:set({ kind = "kick", id = member.id, name = member.name })
				end, string.format(_("Remove %s from the room"), member.name))
			end
			extra[1] = builtin.FloatingLayoutChild{ h = 0.03, v = 0.06, item = item }
		end
		if #cells > 0 then cells[#cells + 1] = gap(12) end
		cells[#cells + 1] = lobby.memberCard(member, playing, extra)
		if #cells >= 3 then flush() end
	end
	flush()

	local lines = state.chat or {}
	local chatRows = {}
	local first = math.max(1, #lines - 40)
	for i = first, #lines do
		local line = lines[i]
		chatRows[#chatRows + 1] = row({
			label(line.from .. ":", line.you and "font-scale-body, info" or "font-scale-body, success"),
			gap(6),
			label(line.text, "font-scale-body"),
		})
		chatRows[#chatRows + 1] = gap(3)
	end
	if #chatRows == 0 then chatRows[1] = gap(1) end
	local function sendChat()
		local msg = chatText:get()
		if msg and not msg:match("^%s*$") then
			chatText:set("")
			send({ action = "chat", text = msg }, nil)
		end
	end

	-- The save the room starts from, in a line.
	local start, upload, pick = room.start, room.upload, pickS:old()
	local startText
	if pick then
		startText = string.format(_("Reading %s..."), pick.save)
	elseif upload then
		startText = string.format(_("Sending %s to the room: %d%%"), upload.save, upload.percent)
	elseif start then
		startText = lobby.startLine(start)
	elseif room.you_own then
		startText = _("A new world: Set up world creates its map and settings")
	else
		startText = _("The world the owner's game has")
	end

	-- The room tab: the world and the players on the left, as the game's
	-- Load Game page shows a save; the room's card, the chat and what the
	-- player can do on the right.
	-- The owner picks the save, its mods and their settings by clicking its
	-- picture, as a save's tile opens it on the game's Load Game page.
	-- A card as the players' are, the main menu's: the world's picture with
	-- its name, and what a click does, on the card's band.
	local pickable = room.you_own and not playing and onPick ~= nil
	local preview = pictureCard(lobby.startPicture(room), startText,
		pickable and _("Click to change the save and mods") or "", nil,
		pickable and function()
			confirmS:set(nil)
			pickWorld()
		end or nil,
		not pickable or (canAct and not busy and pick == nil), SIZE.ROOM_LEFT - 24, SIZE.PREVIEW_HEIGHT, nil, "bottom-left")
	local previewChildren = { preview }
	if upload then
		previewChildren[#previewChildren + 1] = gap(4)
		previewChildren[#previewChildren + 1] = builtin.Component{
			meta = { styleSheet = style{ size = { SIZE.ROOM_LEFT - 24, 10 } } },
			layout = builtin.BoxLayout{ children = { builtin.ProgressBar{ value = math.min(1, upload.percent / 100) } } },
		}
	end
	-- Two columns of the game's cards, each a card with its title on top and
	-- one below it, the two rows as high on both sides: the world and the
	-- players on the left, the room and its chat on the right.
	local left = column({
		native.card(_("World"), {
			column(previewChildren, style{ size = { SIZE.ROOM_LEFT - 24, SIZE.PREVIEW_HEIGHT } }),
		}),
		gap(8),
		native.card(string.format(_("Players  ·  %d of %d  ·  %d ready"), #room.members, room.max_players,
			readyCount(room)), {
			builtin.ScrollArea{
				meta = { styleSheet = style{ size = { SIZE.ROOM_LEFT - 24, SIZE.PLAYERS_HEIGHT } } },
				horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
				verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
				content = column(memberRows),
			},
		}),
	}, style{ size = { SIZE.ROOM_LEFT, AUTO } })

	-- The room: its invite code first, as what its owner hands out, then
	-- the rest in lines, and how this player's mods stand, which leads to
	-- them.
	local modsLine, modsTone = lobby.roomModsLine(state)
	local info = {
		note(_("Invite code")),
		row({
			label(room.invite ~= "" and inviteCode(room.invite) or "-", "font-scale-title-2, info"),
			gui_react_util.makeHorizontalSpacer(),
			-- The hook puts it on the clipboard (the game's GUI has no
			-- clipboard of its own); "Copied" for a moment after.
			button(copiedS:old() > 0 and _("Copied") or _("Copy"), function()
				local refused = act({ action = "copy", text = inviteCode(room.invite) })
				refusedS:set(refused)
				if not refused then copiedS:set(COPIED_POLLS) end
			end, nil, room.invite ~= "", _("Copy the invite code, to paste it to your friends")),
		}, style{ size = { SIZE.ROOM_RIGHT - 44, 44 } }),
		gap(10),
		native.entry(_("Players"), label(string.format(_("%d of %d  ·  %d ready"), #room.members, room.max_players,
			readyCount(room)))),
		native.entry(_("Play style"), label(room.competitive and _("Competitive") or _("Co-op"))),
		native.entry(_("Password"), label(room.has_password and _("Yes") or _("No"))),
		native.entry(_("Server"), label(serverName(state))),
		native.entry(_("Mods"), modsLine and button(modsLine, function() roomTabS:set("mods") end,
			modsTone == "error" and "primary" or nil, true, _("The room's mods, and yours"))
			or label(_("None named yet"))),
	}
	local right = column({
		native.card(room.name, {
			column(info, style{ size = { SIZE.ROOM_RIGHT - 24, SIZE.PREVIEW_HEIGHT } }),
		}),
		gap(8),
		native.card(_("Chat"), {
			column({
				builtin.ScrollArea{
					meta = { styleSheet = style{ size = { SIZE.ROOM_RIGHT - 24, SIZE.PLAYERS_HEIGHT - 48 } } },
					horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
					verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
					content = column(chatRows),
				},
				gap(8),
				row({
					input(chatText, _("Say something to the room"), SIZE.ROOM_RIGHT - 130,
						{ maxLength = 280, acceptOnFocusLoss = false, onEnter = function() sendChat() end }),
					gap(8),
					button(_("Send"), sendChat, nil, canAct),
				}, style{ size = { SIZE.ROOM_RIGHT - 24, 40 } }),
			}, style{ size = { SIZE.ROOM_RIGHT - 24, SIZE.PLAYERS_HEIGHT } }),
		}),
	}, style{ size = { SIZE.ROOM_RIGHT, AUTO } })

	local missing = (tonumber(state.room_mods_missing) or 0) + (tonumber(state.room_mods_other) or 0)
	local roomModCount = #(state.room_mods or {}) + (tonumber(state.room_mods_more) or 0)
	local ownCount = 0
	for _i, m in ipairs(state.mods or {}) do
		if m.choosable then ownCount = ownCount + 1 end
	end
	local tabs = {
		native.tabOf("room", _("Room"), row({ left, gap(16), right })),
		native.tabOf("mods", missing > 0 and string.format(_("The room's mods (%d) · %d missing"), roomModCount,
			missing) or string.format(_("The room's mods (%d)"), roomModCount), roomModsPanel()),
		native.tabOf("own", string.format(_("Only for you (%d)"), ownCount), ownModsPanel()),
	}

	-- At the bottom left, small and apart: leaving the room, a new world
	-- instead of a save, and the owner's taking back that they are ready.
	-- At the bottom right, alone, as the game's Load Game button: what
	-- moves the room on.
	local smalls = {}
	local function small(item)
		if #smalls > 0 then smalls[#smalls + 1] = gap(8) end
		smalls[#smalls + 1] = item
	end
	if confirm and confirm.kind == "leave" then
		small(label(_("Leave the room?"), "font-scale-body, warning"))
		small(native.foot(_("Leave"), function()
			pageS:set("choose")
			send({ action = "leave" }, _("Leaving the room..."))
		end, "error-tape", canAct))
		small(native.foot(_("Stay"), function() confirmS:set(nil) end))
	else
		small(native.foot(_("Leave room"), function() confirmS:set({ kind = "leave" }) end, "error-tape", canAct))
	end
	-- The small ones on the left, the button at the right edge. The ready
	-- button says where the player stands, Ready or Not ready, and a click
	-- turns it over; the owner's, once ready, is just before Start.
	local footer = { row(smalls) }
	footer[#footer + 1] = gui_react_util.makeHorizontalSpacer()
	local function readyButton()
		return native.foot(_("Ready"), function()
			send({ action = "ready", ready = false }, nil)
		end, "secondary", canAct and not busy, _("You are ready. Click if you are not."))
	end
	if not playing and me and me.ready and room.you_own then
		footer[#footer + 1] = readyButton()
	end
	if playing then
		footer[#footer + 1] = note(_("The room's game is under way."))
	elseif room.you_own and not (me and me.ready) and not state.start_save and onNewGame then
		footer[#footer + 1] = native.wide(_("Set up world"), onNewGame, canAct and not busy,
			_("Choose the map and settings, then start multiplayer"))
	elseif room.you_own and me and me.ready then
		local all = everyoneReady(room)
		local waits = lobby.startWaits(room) or pickS:old() ~= nil
		-- A player whose mods differ cannot start with the room (the server
		-- refuses): said before, not after.
		local differs
		for _i, member in ipairs(room.members) do
			local how = lobby.memberDiffers(member)
			if how and not differs then differs = string.format("%s: %s", member.name, how) end
		end
		footer[#footer + 1] = native.wide(_("Start the game"), function()
			send({ action = "start" }, _("Starting the room's game..."))
		end, canAct and all and not waits and not differs and not busy,
			(waits and _("The save is still on its way to the room"))
				or differs
				or (all and _("Every player's game loads the room's world")) or _("Waiting for everyone to be ready"))
	elseif me and me.ready then
		footer[#footer + 1] = readyButton()
	else
		footer[#footer + 1] = native.wide(_("Not ready"), function()
			send({ action = "ready", ready = true }, _("Getting ready..."))
		end, canAct and not busy, _("You are not ready. Click when you are."))
	end

	return frame(_("Your room"), status, builtin.TabWidget{
		orientation = builtin.type.TabOrientation.North,
		deselectAllowed = false,
		showIndicators = true,
		value = roomTabS:old(),
		tabs = tabs,
		onValueChange = function(value) roomTabS:set(value) end,
	}, footer)
end

-- The live line under a Multiplayer card on the main menu, from the lobby
-- the hook has: its own recipe, so only it redraws when the lobby changes.
lobby.CardLine = react.RegisterRecipe("Tpf3mpCardLine", function(params)
	local lineS = react.useState(lobby.summary(nil))
	react.onStepTimer(function()
		local state = fetchState()
		local line = params and params.line and params.line(state) or lobby.summary(state)
		if line ~= lineS:old() then lineS:set(line) end
	end, 1.0, false)
	-- A recipe placed among a layout's children must return a layout: the
	-- game refused a bare TextView here ("Recipe child must be a layout",
	-- ReactFramework::Load, 2026-09-30), as its own recipes return one.
	return builtin.BoxLayout{
		children = {
			builtin.TextView{
				meta = { class = "font-scale-annotation, annotation" },
				text = lineS:old(),
			},
		},
	}
end)

-- What the "Join a friend" card says: the room once in one.
function lobby.joinLine(state)
	local room = state and state.room
	if room and room.invite ~= "" then
		return string.format(_("Your room: invite %s"), inviteCode(room.invite))
	end
	return _("With the invite code they send you")
end

return lobby
