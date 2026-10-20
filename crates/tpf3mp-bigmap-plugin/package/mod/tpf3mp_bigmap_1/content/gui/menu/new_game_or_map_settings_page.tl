-- TPF3-MP begin: the game's files by absolute path
-- TPF3-MP: the game's gui/menu/new_game_or_map_settings_page.tl with big
-- maps' size rows added to the size dropdown and its ratios 1:6 to 1:10 to
-- the ratio dropdown (docs/BIGMAPS.md, "Stage 1" and "Longer ratios").
-- The hook serves this copy in place of the game's file and falls back to
-- the game's own page if it does not load. Each change is a marked block,
-- the game's lines it replaces kept under "was" as comments: re-apply them
-- to a patched game file with `cargo run -p tpf3mp-bigmap -- page <file>`
-- (crates/tpf3mp-bigmap/src/page.rs), never by hand. After silver2127's
-- Big Maps for TPF2 (tpf2-bigmap), whose native size detour this replaces.
-- Paths are absolute ("::/"): a leading-slash or relative path would be
-- looked up in this mod's folder.
local builtin = ug_require "::/gui/main/builtin.lua" as Builtin
local content_card = ug_require "::/gui/main/content_card.tl" as ContentCard
local difficulty_util = ug_require "::/base/difficulty_util.tl" as DifficultyUtil
local lang_util = ug_require "::/scripts/lang_util.tl" as LangUtil
local menu_icon_react_util = ug_require "::/gui/menu/menu_icon_react_util.tl" as MenuIconReactUtil
local new_game_react_util = ug_require "::/gui/menu/new_game_react_util.tl" as NewGameReactUtil
local react = ug_require "::/gui/main/react.lua" as React
local savegame_react_util = ug_require "::/gui/menu/savegame_react_util.tl" as SavegameReactUtil
local callout_react_util = ug_require "::/gui/main/callout_react_util.tl" as CalloutReactUtil
local script_param_util = ug_require "::/gui/main/script_param_util.tl" as ScriptParamUtil
local table_util = ug_require "::/scripts/table_util.tl" as TableUtil

local GameplaySettings = ug_require "::/gui/menu/advanced_settings.tl" as Recipe<IMenuPageParam>
local ModSelectorPage = ug_require "::/gui/menu/mod_selector_page.tl" as Recipe<IMenuPageParam>
-- TPF3-MP was:
--local builtin = ug_require "/gui/main/builtin.lua" as Builtin
--local content_card = ug_require "::/gui/main/content_card.tl" as ContentCard
--local difficulty_util = ug_require "/base/difficulty_util.tl" as DifficultyUtil
--local lang_util = ug_require "/scripts/lang_util.tl" as LangUtil
--local menu_icon_react_util = ug_require "/gui/menu/menu_icon_react_util.tl" as MenuIconReactUtil
--local new_game_react_util = ug_require "new_game_react_util.tl" as NewGameReactUtil
--local react = ug_require "/gui/main/react.lua" as React
--local savegame_react_util = ug_require "/gui/menu/savegame_react_util.tl" as SavegameReactUtil
--local callout_react_util = ug_require "/gui/main/callout_react_util.tl" as CalloutReactUtil
--local script_param_util = ug_require "/gui/main/script_param_util.tl" as ScriptParamUtil
--local table_util = ug_require "/scripts/table_util.tl" as TableUtil
--
--local GameplaySettings = ug_require "advanced_settings.tl" as Recipe<IMenuPageParam>
--local ModSelectorPage = ug_require "mod_selector_page.tl" as Recipe<IMenuPageParam>
-- TPF3-MP end

-- TPF3-MP begin: big maps' rows, from the mod tpf3mp_bigmap_1
-- The ladder and the logic are plain Lua in this mod
-- (scripts/tpf3mp_bigmap/, tested in crates/tpf3mp-bigmap/tests/mod_lua.rs).
-- Should they not load, the page offers the game's own sizes only.
local record BigmapRow
	label : string
	tiles : integer
	peakMb : number
	shapes : any
end

local record BigmapMenu
	ramMb : function() : number
	offered : function(ladder : {BigmapRow}, ramMb : number) : {BigmapRow}, {string}
	sizeValues : function(stockValues : {string}, rows : {BigmapRow}) : {string}
	indexOf : function(value : number, pick : integer, stockCount : integer, stockNumbers : {number}, rowCount : integer) : integer
	choose : function(index : integer, stockCount : integer, stockNumbers : {number}) : number, integer
	gameTiles : function(rows : {BigmapRow}, pick : integer, ratioIndex : integer) : any, any
	densityChoices : function(stockValues : {string}, ladder : {BigmapRow}) : {string}, {number}
	STOCK_DENSITY_LEVELS : integer
	STOCK_RATIOS : integer
	extraRatios : function(shapes : any, peakMb : any, ramMb : number, label : any) : {integer}, string
	stockShapes : function(ladder : {BigmapRow}, squareTiles : integer) : any, any, any
	formatValues : function(stockValues : {string}, offered : {integer}) : {string}
	formatIndexOf : function(value : number, ratioPick : integer, stockCount : integer, stockNumbers : {number}, offered : {integer}) : integer
	formatChoose : function(index : integer, stockCount : integer, stockNumbers : {number}, offered : {integer}) : number, integer
	extraTiles : function(shapes : any, ratioPick : integer) : any, any
end

local bigmapMenuLoaded, bigmapMenuValue = pcall(ug_require, "tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/menu.lua")
local bigmapLadderLoaded, bigmapLadderValue = pcall(ug_require, "tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/ladder.lua")
local bigmap : BigmapMenu = nil
local bigmapLadder : {BigmapRow} = nil
if bigmapMenuLoaded and bigmapLadderLoaded and bigmapMenuValue ~= nil and bigmapLadderValue ~= nil then
	bigmap = bigmapMenuValue as BigmapMenu
	bigmapLadder = bigmapLadderValue as {BigmapRow}
	pcall(debugPrint, "[tpf3mp] big maps: TPF3-MP new_game_or_map_settings_page.tl is in effect")
else
	local reason = bigmapMenuLoaded and bigmapLadderValue or bigmapMenuValue
	pcall(debugPrint, "[tpf3mp] big maps: the mod's scripts did not load (" .. tostring(reason) .. "); the sizes are the game's own")
end

-- The added row picked (1 is the first), 0 for one of the game's own. It
-- lasts the session; the page's state carries it between renders.
local bigmapPick = 0
local bigmapPickState : ReactStateT<integer> = nil
-- The added rows this machine has the memory to generate, and why any
-- other is missing.
local bigmapRows : {BigmapRow} = {}
local bigmapNotes : {string} = {}

local function bigmapRefresh()
	bigmapRows = {}
	bigmapNotes = {}
	if bigmap == nil then
		return
	end
	local ok, rows, notes = pcall(function() : {BigmapRow}, {string}
		return bigmap.offered(bigmapLadder, bigmap.ramMb())
	end)
	if ok then
		bigmapRows = rows as {BigmapRow}
		bigmapNotes = notes as {string}
	else
		pcall(debugPrint, "[tpf3mp] big maps: " .. tostring(rows) .. "; the sizes are the game's own")
	end
end

-- The added ratio picked (1 is 1:6), 0 for one of the game's own; page
-- state as the row picked is.
local bigmapRatio = 0
local bigmapRatioState : ReactStateT<integer> = nil
-- Set while the game's own 1:1 square is asked for, so the answer is the
-- game's.
local bigmapInSquare = false

-- The tiles of the added row picked, or of the added ratio picked, at the
-- ratio `format` (0-based, as getNumTiles has it); nil for the game's own.
-- `square` asks the game for the size's 1:1 square.
local function bigmapNumTiles(format : integer, square : function() : Vec2i) : Vec2i
	if bigmap == nil or bigmapInSquare or (bigmapPick < 1 and bigmapRatio < 1) then
		return nil
	end
	local ok, x, y : boolean, any, any = false, nil, nil
	if bigmapPick >= 1 then
		local ratioIndex = format
		if bigmapRatio >= 1 then
			ratioIndex = bigmap.STOCK_RATIOS - 1 + bigmapRatio
		end
		ok, x, y = pcall(bigmap.gameTiles, bigmapRows, bigmapPick, ratioIndex)
	else
		bigmapInSquare = true
		local got, size = pcall(square)
		bigmapInSquare = false
		if not got or size == nil then
			return nil
		end
		local shapes = bigmap.stockShapes(bigmapLadder, (size as Vec2i).x)
		ok, x, y = pcall(bigmap.extraTiles, shapes, bigmapRatio)
	end
	if not ok or type(x) ~= "number" or type(y) ~= "number" then
		pcall(debugPrint, "[tpf3mp] big maps: no shape for row " .. tostring(bigmapPick) .. " at ratio " .. tostring(format) .. "/" .. tostring(bigmapRatio) .. " (" .. tostring(y or x) .. "); the game's own size is used")
		return nil
	end
	return api.type.Vec2i.new(x as integer, y as integer)
end

-- The size dropdown: the game's rows, then the added rows this machine can
-- generate. A game row sets "map.size" as the game's own dropdown does. An
-- added row sets it to the game's largest size, so the save and every other
-- reader of "map.size" see a value the game knows, and keeps its own pick
-- here. A line under the dropdown says why a row is missing.
local function bigmapAddMapSizeSettingsEntry(settings : {NewGameReactUtil.SettingsEntry}, activeModsParamsState : ReactStateT<{string : {string : integer}}>, filterTags : {string})
	local scriptParam = script_param_util.getScriptParam("map.size", filterTags)
	if bigmap == nil or scriptParam == nil or bigmapPickState == nil or #bigmapRows == 0 then
		bigmapPick = 0
		new_game_react_util.addMapSizeSettingsEntry(settings, activeModsParamsState, filterTags)
	else
		local stockCount = #scriptParam.values
		local stockNumbers = scriptParam.numbers
		local paramForUi : ScriptParamUtil.ParamForUi = {
			uiType = scriptParam.uiType,
			defaultIndex = scriptParam.defaultIndex,
			name = scriptParam.name,
			values = bigmap.sizeValues(scriptParam.values, bigmapRows),
			numbers = nil,
			tooltips = nil,
			allowCoalesce = true,
		}
		local element = script_param_util.buildScriptParamCompSimple({
			scriptParam = paramForUi,
			currentValue = bigmap.indexOf(activeModsParamsState:old()[""]["map.size"], bigmapPick, stockCount, stockNumbers, #bigmapRows),
			onValueChange = function(value : number)
				local mapSize, pick = bigmap.choose(math.floor(value), stockCount, stockNumbers)
				bigmapPick = pick
				bigmapPickState:set(pick)
				local copy = table_util.copy(activeModsParamsState:old())
				copy[""]["map.size"] = mapSize as integer
				if not table_util.deepEquals(copy, activeModsParamsState:old()) then
					activeModsParamsState:set(copy)
				end
			end,
			toggleButtonsFlowLayout = false,
			vertical = false,
			addSpacer = true,
		})
		table.insert(settings, {
			title = scriptParam.name,
			description = scriptParam.tooltip,
			hintIdKey = "hintIdKey" .. scriptParam.name,
			element = element,
		})
	end
	if bigmap ~= nil and #bigmapNotes > 0 then
		table.insert(settings, {
			title = "",
			description = "",
			element = script_param_util.wrap(
				_("Big maps"),
				false,
				builtin.TextView{
					meta = { class = "right-parameters, font-scale-body" },
					text = table.concat(bigmapNotes, "\n"),
				},
				true
			),
		})
	end
end

-- TPF3-MP end
-- TPF3-MP begin: big maps' density levels

-- A density slider (towns, or industries with their runtime target) with
-- big maps' levels before the game's own: "-" to "----", one dash per size row
-- (scripts/tpf3mp_bigmap/menu.lua). The level is stored where the game's
-- own slider stores it; the hook's difficulty_util wrap gives it its
-- factor. Without the mod's scripts, or a slider of another length, the
-- game's own slider.
local bigmapDensityNoted : {string : boolean} = {}
local function bigmapAddDensitySettingsEntry(settings : {NewGameReactUtil.SettingsEntry}, activeModsParamsState : ReactStateT<{string : {string : integer}}>, key : string, filterTags : {string}, otherKeys : {string})
	local scriptParam = script_param_util.getScriptParam(key, filterTags)
	if bigmap == nil or bigmapLadder == nil or #bigmapLadder == 0 or scriptParam == nil or #scriptParam.values ~= bigmap.STOCK_DENSITY_LEVELS then
		pcall(debugPrint, "[tpf3mp] big maps: " .. key .. " keeps the game's slider")
		new_game_react_util.searchBuildAndAddScriptParamComp(settings, activeModsParamsState, key, false, filterTags, otherKeys)
		return
	end
	local values, numbers = bigmap.densityChoices(scriptParam.values, bigmapLadder)
	if not bigmapDensityNoted[key] then
		bigmapDensityNoted[key] = true
		pcall(debugPrint, "[tpf3mp] big maps: " .. key .. " has " .. tostring(#values) .. " levels")
	end
	local paramForUi : ScriptParamUtil.ParamForUi = {
		uiType = scriptParam.uiType,
		defaultIndex = scriptParam.defaultIndex + #bigmapLadder,
		name = scriptParam.name,
		values = values,
		numbers = numbers,
		tooltips = nil,
		allowCoalesce = true,
	}
	local element = script_param_util.buildScriptParamCompSimple({
		scriptParam = paramForUi,
		currentValue = activeModsParamsState:old()[""][key],
		onValueChange = function(value : number)
			local level = math.floor(value)
			local copy = table_util.copy(activeModsParamsState:old())
			copy[""][key] = level
			for __, other in ipairs(otherKeys or {}) do
				copy[""][other] = level
			end
			if not table_util.deepEquals(copy, activeModsParamsState:old()) then
				activeModsParamsState:set(copy)
			end
		end,
		toggleButtonsFlowLayout = false,
		vertical = false,
		addSpacer = false,
	})
	table.insert(settings, {
		title = scriptParam.name,
		description = scriptParam.tooltip .. "\n\n- to ----: fewer, for the big map sizes.",
		hintIdKey = "hintIdKey" .. scriptParam.name,
		element = element,
	})
end
-- TPF3-MP end
-- zero-based getter -.-
local function getNumTiles(size : integer, format : integer) : Vec2i
	local filterTags = new_game_react_util.getFilterTags()
	local mapSizeParam = script_param_util.getScriptParam("map.size", filterTags)
	local mapFormatParam = script_param_util.getScriptParam("map.format", filterTags)

	-- size and format indices come from baseModParams, so are 1-based instead of 0-based 
	size = script_param_util.clampScriptParamValueIndex(size, mapSizeParam) - 1
	format = script_param_util.clampScriptParamValueIndex(format, mapFormatParam) - 1
	-- TPF3-MP begin: an added size row answers for itself
	local bigmapSize = bigmapNumTiles(format, function() : Vec2i
		return getNumTiles(size + 1, 1)
	end)
	if bigmapSize ~= nil then
		return bigmapSize
	end
	-- TPF3-MP end

	if size == 0 then -- "tiny"
		if format == 0 then return api.type.Vec2i.new(16, 16) end
		if format == 1 then return api.type.Vec2i.new(10, 20) end
		if format == 2 then return api.type.Vec2i.new(8, 24) end
		if format == 3 then return api.type.Vec2i.new(8, 24) end
		if format == 4 then return api.type.Vec2i.new(6, 30) end
	elseif size == 1 then -- "small"
		if format == 0 then return api.type.Vec2i.new(32, 32) end
		if format == 1 then return api.type.Vec2i.new(22, 44) end
		if format == 2 then return api.type.Vec2i.new(18, 54) end
		if format == 3 then return api.type.Vec2i.new(16, 54) end
		if format == 4 then return api.type.Vec2i.new(14, 70) end
	elseif size == 2 then -- "medium"
		if format == 0 then return api.type.Vec2i.new(44, 44) end
		if format == 1 then return api.type.Vec2i.new(32, 64) end
		if format == 2 then return api.type.Vec2i.new(26, 78) end
		if format == 3 then return api.type.Vec2i.new(22, 88) end
		if format == 4 then return api.type.Vec2i.new(20, 100) end
	elseif size == 3 then -- "large"
		if format == 0 then return api.type.Vec2i.new(56, 56) end
		if format == 1 then return api.type.Vec2i.new(40, 80) end
		if format == 2 then return api.type.Vec2i.new(32, 96) end
		if format == 3 then return api.type.Vec2i.new(28, 112) end
		if format == 4 then return api.type.Vec2i.new(24, 126) end
	elseif size == 4 then -- "veryLarge"
		if format == 0 then return api.type.Vec2i.new(64, 64) end
		if format == 1 then return api.type.Vec2i.new(44, 88) end
		if format == 2 then return api.type.Vec2i.new(36, 108) end
		if format == 3 then return api.type.Vec2i.new(32, 128) end
		if format == 4 then return api.type.Vec2i.new(28, 140) end
	elseif size == 5 then -- "huge"
		if format == 0 then return api.type.Vec2i.new(80, 80) end
		if format == 1 then return api.type.Vec2i.new(56, 112) end
		if format == 2 then return api.type.Vec2i.new(46, 138) end
		if format == 3 then return api.type.Vec2i.new(40, 160) end
		if format == 4 then return api.type.Vec2i.new(34, 170) end
	elseif size == 6 then -- "megalomaniac"
		if format == 0 then return api.type.Vec2i.new(96, 96) end
		if format == 1 then return api.type.Vec2i.new(66, 132) end
		if format == 2 then return api.type.Vec2i.new(54, 162) end
		if format == 3 then return api.type.Vec2i.new(48, 192) end
		if format == 4 then return api.type.Vec2i.new(42, 210) end
	elseif size == 7 then -- "colossal"
		if format == 0 then return api.type.Vec2i.new(112, 112) end
		if format == 1 then return api.type.Vec2i.new(80, 160) end
		if format == 2 then return api.type.Vec2i.new(64, 192) end
		if format == 3 then return api.type.Vec2i.new(56, 224) end
		if format == 4 then return api.type.Vec2i.new(50, 250) end
	end

	return api.type.Vec2i.new(16, 16)
end

-- TPF3-MP begin: big maps' longer ratios, 1:6 to 1:10
-- The ratio dropdown: the game's ratios, then 1:6 to 1:10 where the size
-- picked can be built at them on this machine (scripts/tpf3mp_bigmap/
-- menu.lua). An added ratio sets "map.format" to the game's last ratio, so
-- the save and every other reader see a value the game knows, and keeps
-- its own pick here. A line under the dropdown says why a ratio is missing.
-- Without the mod's scripts, or a ratio list other than the game's five,
-- the game's own dropdown.
local function bigmapAddMapFormatSettingsEntry(settings : {NewGameReactUtil.SettingsEntry}, activeModsParamsState : ReactStateT<{string : {string : integer}}>, filterTags : {string})
	local scriptParam = script_param_util.getScriptParam("map.format", filterTags)
	if bigmap == nil or scriptParam == nil or bigmapRatioState == nil or #scriptParam.values ~= bigmap.STOCK_RATIOS then
		bigmapRatio = 0
		new_game_react_util.addMapFormatSettingsEntry(settings, activeModsParamsState, filterTags)
		return
	end
	local shapes, peakMb, label : any, any, any = nil, nil, nil
	if bigmapPick >= 1 and bigmapRows[bigmapPick] ~= nil then
		local row = bigmapRows[bigmapPick]
		shapes, peakMb, label = row.shapes, row.peakMb, row.label
	else
		bigmapInSquare = true
		local got, square = pcall(getNumTiles, activeModsParamsState:old()[""]["map.size"], 1)
		bigmapInSquare = false
		if got and square ~= nil then
			shapes, peakMb, label = bigmap.stockShapes(bigmapLadder, (square as Vec2i).x)
		end
	end
	local ok, offered, note = pcall(bigmap.extraRatios, shapes, peakMb, bigmap.ramMb(), label)
	if not ok then
		pcall(debugPrint, "[tpf3mp] big maps: " .. tostring(offered) .. "; the game's own ratios")
		bigmapRatio = 0
		new_game_react_util.addMapFormatSettingsEntry(settings, activeModsParamsState, filterTags)
		return
	end
	local ratios = offered as {integer}
	local still = false
	for __, pick in ipairs(ratios) do
		if pick == bigmapRatio then
			still = true
		end
	end
	if not still then
		bigmapRatio = 0
	end
	local stockCount = #scriptParam.values
	local stockNumbers = scriptParam.numbers
	local paramForUi : ScriptParamUtil.ParamForUi = {
		uiType = scriptParam.uiType,
		defaultIndex = scriptParam.defaultIndex,
		name = scriptParam.name,
		values = bigmap.formatValues(scriptParam.values, ratios),
		numbers = nil,
		tooltips = nil,
		allowCoalesce = true,
	}
	local element = script_param_util.buildScriptParamCompSimple({
		scriptParam = paramForUi,
		currentValue = bigmap.formatIndexOf(activeModsParamsState:old()[""]["map.format"], bigmapRatio, stockCount, stockNumbers, ratios),
		onValueChange = function(value : number)
			local mapFormat, pick = bigmap.formatChoose(math.floor(value), stockCount, stockNumbers, ratios)
			bigmapRatio = pick
			bigmapRatioState:set(pick)
			local copy = table_util.copy(activeModsParamsState:old())
			copy[""]["map.format"] = mapFormat as integer
			if not table_util.deepEquals(copy, activeModsParamsState:old()) then
				activeModsParamsState:set(copy)
			end
		end,
		toggleButtonsFlowLayout = false,
		vertical = false,
		addSpacer = true,
	})
	table.insert(settings, {
		title = scriptParam.name,
		description = scriptParam.tooltip,
		hintIdKey = "hintIdKey" .. scriptParam.name,
		element = element,
	})
	if note ~= nil then
		table.insert(settings, {
			title = "",
			description = "",
			element = script_param_util.wrap(
				_("Big maps"),
				false,
				builtin.TextView{
					meta = { class = "right-parameters, font-scale-body" },
					text = tostring(note),
				},
				true
			),
		})
	end
end

-- TPF3-MP end
local createMap = function(baseModParams : {string:integer}, configDict : {{string, string}}) : GameMap
	local map = api.type.Map.new()
	map.waterLevel = 0.0

	local levels = 6
	local numTiles = getNumTiles(baseModParams["map.size"], baseModParams["map.format"])
	local dim = 1 << levels

	map.numTilesX = numTiles.x
	map.numTilesY = numTiles.y
	map.levels = levels
	map.resolution = api.type.Vec3f.new(4.0, 4.0, 0.05)
	map.offsetZ = -100.0
	map.waterLevel = 0.0

	map.climateId = table_util.find(configDict, "climate")

	local fullWidth = numTiles.x * dim + 1
	local fullHeight = numTiles.y * dim + 1
	
	map:setHeightMap(fullWidth * fullHeight, 5.0)
	map:setBiomeMap(numTiles.x * dim, numTiles.y * dim, 0)
	
	return map
end

local record SettingsAndTitle
	settings : {NewGameReactUtil.SettingsEntry}
	title : string
	description : string
end

local interface SettingsStrategy
	buildSettings : function(
		possibleGenerators : {NewGameReactUtil.Generator},
		saveGameData : SaveGameData,
		mapState : ReactStateT<GameMap>,
		mapParamsState : ReactStateT<NewGameReactUtil.NewGameState>,
		mapGenParamsState : ReactStateT<{string : integer}>,
		configDictState : ReactStateT<{{string, string}}>,
		activeModsParamsState : ReactStateT<{string : {string : integer}}>
	) : {SettingsAndTitle}
	getTitle : function() : string
	styleClasses : string
end

local newGameNewMapStrategy : SettingsStrategy = {
	getTitle = function() : string
		return _("New Game - Settings")
	end,
	buildSettings = function(
		possibleGenerators : {NewGameReactUtil.Generator},
		_saveGameData : SaveGameData,
		mapState : ReactStateT<GameMap>,
		mapParamsState : ReactStateT<NewGameReactUtil.NewGameState>,
		mapGenParamsState : ReactStateT<{string : integer}>,
		configDictState : ReactStateT<{{string, string}}>,
		activeModsParamsState : ReactStateT<{string : {string : integer}}>
	) : {SettingsAndTitle}

		local filterTags = new_game_react_util.getFilterTags()

		local settingsWorld : {NewGameReactUtil.SettingsEntry} = {}
		-- TPF3-MP begin: the size dropdown with the added rows
		bigmapAddMapSizeSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP was:
--		new_game_react_util.addMapSizeSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP end
		-- TPF3-MP begin: the ratio dropdown with the longer ratios
		bigmapAddMapFormatSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP was:
--		new_game_react_util.addMapFormatSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP end
		new_game_react_util.addGeneratorSettingsEntry(settingsWorld, possibleGenerators, mapState, mapParamsState)
		new_game_react_util.addSeedSettingsEntry(settingsWorld, mapState, mapParamsState)

		local settingsTerrain : {NewGameReactUtil.SettingsEntry} = {}
		new_game_react_util.addTerrainParameterSettingsEntry(settingsTerrain, mapState, mapParamsState, mapGenParamsState)

		local settingsCiv : {NewGameReactUtil.SettingsEntry} = {}
		new_game_react_util.addNamesSettingsEntry(settingsCiv, configDictState)
		-- TPF3-MP begin: the town density slider with big maps' levels
		bigmapAddDensitySettingsEntry(settingsCiv, activeModsParamsState, "locations.towns.frequency", filterTags, nil)
		-- TPF3-MP was:
--		new_game_react_util.searchBuildAndAddScriptParamComp(
--			settingsCiv,
--			activeModsParamsState,
--			"locations.towns.frequency",
--			false, -- addSpacer
--			filterTags
--		)
		-- TPF3-MP end
		-- TPF3-MP begin: the industry density slider with big maps' levels
		bigmapAddDensitySettingsEntry(settingsCiv, activeModsParamsState, "locations.industry.initialIndustryDensity", filterTags, {"locations.industry.targetIndustryDensity"})
		-- TPF3-MP was:
--		new_game_react_util.searchBuildAndAddScriptParamComp(
--			settingsCiv,
--			activeModsParamsState,
--			"locations.industry.initialIndustryDensity",
--			false, -- addSpacer
--			filterTags,
--			{"locations.industry.targetIndustryDensity"}
--		)
		-- TPF3-MP end
		new_game_react_util.addStartYearSettingsEntry(settingsCiv, mapState, mapParamsState)

		local settingsEnv : {NewGameReactUtil.SettingsEntry} = {}
		new_game_react_util.addInitialWeatherSettingsEntry(settingsEnv, activeModsParamsState, filterTags)
		new_game_react_util.addInitialTimeOfDaySettingsEntry(settingsEnv, activeModsParamsState, filterTags)
		new_game_react_util.addTutorialSettingsEntry(settingsEnv, activeModsParamsState, filterTags)

		return {
			{
				settings = settingsWorld,
				title = _("World"),
				description = _("Settings to configure the world."),
			},
			{
				settings = settingsTerrain,
				title = _("Terrain"),
				description = _("Settings to configure the terrain."),
			},
			{
				settings = settingsCiv,
				title = _("Civilization"),
				description = _("Settings to configure the civilization."),
			},
			{
				settings = settingsEnv,
				title = _("Environment"),
				description = _("Settings to configure the environment."),
			},
		}
	end,
	styleClasses = "generator",
}

local newGameLoadMapStrategy : SettingsStrategy = {
	getTitle = function() : string
		return _("New Game - Load Map")
	end,
	buildSettings = function(
		_possibleGenerators : {NewGameReactUtil.Generator},
		savegameData : SaveGameData,
		_mapState : ReactStateT<GameMap>,
		_mapParamsState : ReactStateT<NewGameReactUtil.NewGameState>,
		_mapGenParamsState : ReactStateT<{string : integer}>,
		configDictState : ReactStateT<{{string, string}}>,
		_activeModsParamsState : ReactStateT<{string : {string : integer}}>
	) : {SettingsAndTitle}

		local configDict = configDictState:old()

		local formatText = script_param_util.getScriptParamLabelUnfiltered("map.format", savegameData.info.modParams[""]["map.format"])
		local sizeText = script_param_util.getScriptParamLabelUnfiltered("map.size", savegameData.info.modParams[""]["map.size"])

		local settingsWorld : {NewGameReactUtil.SettingsEntry} = {}
		table.insert(settingsWorld, {
			title = "",
			description = "",
			element = script_param_util.wrap(
				_("Map Size"),
				false,
				builtin.TextView{
					meta = {
						class = "right-parameters, font-scale-body"
					},
					text = sizeText,
				},
				true
			),
		})
		table.insert(settingsWorld, {
			title = "",
			description = "",
			element = script_param_util.wrap(
				_("Map Format"),
				false,
				builtin.TextView{
					meta = {
						class = "right-parameters, font-scale-body"
					},
					text = formatText,
				},
				true
			),
		})


		local settingsCiv : {NewGameReactUtil.SettingsEntry} = {}
		local nameListIdx = new_game_react_util.findConfigDictIndex(configDict, "nameList")
		if nameListIdx > 0 then
			local namesResname = configDict[nameListIdx][2]
			local nameId = app.res.nameRep.find(namesResname)
			if nameId > -1 then
				local name = app.res.nameRep.get(nameId).name
				table.insert(settingsCiv, {
					title = "",
					description = "",
					element = script_param_util.wrap(
						_("Names"),
						false,
						builtin.TextView{
							meta = {
								class = "right-parameters, font-scale-body"
							},
							text = name,
						},
						true
					),
				})
			end
		end
		local economiesListIdx = new_game_react_util.findConfigDictIndex(configDict, "economy")
		if economiesListIdx > 0 then
			local economiesResname = configDict[economiesListIdx][2]
			local economyId = app.res.economyRep.find(economiesResname)
			if economyId > -1 then
				local economy = app.res.economyRep.get(economyId)
				table.insert(settingsCiv, {
					title = "",
					description = "",
					element = script_param_util.wrap(
						_("Economy"),
						false,
						builtin.TextView{
							meta = {
								class = "right-parameters, font-scale-body"
							},
							text = economy.desc.name,
						},
						true
					),
				})
			end
		end

		local date = api.type.Date.new(savegameData.info.metadata.startYear, 1, 1)
		table.insert(settingsCiv, {
			title = "",
			description = "",
			element = script_param_util.wrap(
				_("Start Year"),
				false,
				builtin.TextView{
					meta = {
						class = "right-parameters, font-scale-body"
					},
					text = api.util.formatDate(date, api.type["enum"].DateFormat.YEAR)
				},
				true
			),
		})

		return {
			{
				settings = settingsWorld,
				title = _("World"),
			},
			{
				settings = settingsCiv,
				title = _("Civilization"),
			},
		}
	end,
	styleClasses = "load-map",
}

local mapEditorNewMapStrategy : SettingsStrategy = {
	getTitle = function() : string
		return _("Map Editor - New Map")
	end,
	buildSettings = function(
		_possibleGenerators : {NewGameReactUtil.Generator},
		_saveGameData : SaveGameData,
		mapState : ReactStateT<GameMap>,
		mapParamsState : ReactStateT<NewGameReactUtil.NewGameState>,
		_mapGenParamsState : ReactStateT<{string : integer}>,
		configDictState : ReactStateT<{{string, string}}>,
		activeModsParamsState : ReactStateT<{string : {string : integer}}>
	) : {SettingsAndTitle}

		local filterTags = new_game_react_util.getFilterTags()

		local settingsWorld : {NewGameReactUtil.SettingsEntry} = {}
		-- TPF3-MP begin: the size dropdown with the added rows
		bigmapAddMapSizeSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP was:
--		new_game_react_util.addMapSizeSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP end
		-- TPF3-MP begin: the ratio dropdown with the longer ratios
		bigmapAddMapFormatSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP was:
--		new_game_react_util.addMapFormatSettingsEntry(settingsWorld, activeModsParamsState, filterTags)
		-- TPF3-MP end

		local settingsCiv : {NewGameReactUtil.SettingsEntry} = {}
		new_game_react_util.addNamesSettingsEntry(settingsCiv, configDictState)
		new_game_react_util.addStartYearSettingsEntry(settingsCiv, mapState, mapParamsState)

		local settingsEnv : {NewGameReactUtil.SettingsEntry} = {}
		new_game_react_util.addInitialWeatherSettingsEntry(settingsEnv, activeModsParamsState, filterTags)
		new_game_react_util.addInitialTimeOfDaySettingsEntry(settingsEnv, activeModsParamsState, filterTags)

		return {
			{
				settings = settingsWorld,
				title = _("World"),
			},
			{
				settings = settingsCiv,
				title = _("Civilization"),
			},
			{
				settings = settingsEnv,
				title = _("Environment"),
			},
		}
	end,
	styleClasses = "map-editor",
}

local mapEditorLoadMapStrategy : SettingsStrategy = {
	getTitle = function() : string
		return _("Map Editor - Load Map")
	end,
}

local record MapPreviewInputs	
	seed : string
	locationsConfig : BaseConfig.Locations
	economy : string
	mapGenParam : {string : integer}
end

local function makeMapPreviewCompInputs(
	mapParam : NewGameReactUtil.NewGameState,
	mapGenParam : {string : integer},
	configDict : {{string, string}},
	modParams : {string : {string : integer}}
) : MapPreviewInputs
	local result : MapPreviewInputs = {}

	result.seed = mapParam.seed

	local initialIndustryDensityFactor<const> = 1.0

	local locations = api.type.BaseConfig.Locations.new()
	locations.town.maxNumberPerArea = 0.2
	locations.town.allowInRoughTerrain = false
	locations.industry.absoluteMinimum = 24
	locations.industry.industryProductivity = 1
	locations.makeInitialStreets = true
	locations.town.townFrequency = difficulty_util.getScale(
		"locations.towns.frequency",
		modParams[""]["locations.towns.frequency"]
	)
	locations.town.populationDensity = difficulty_util.getScale(
		"locations.towns.populationDensity",
		modParams[""]["locations.towns.populationDensity"]
	)
	locations.industry.industryDensity = 
			initialIndustryDensityFactor * difficulty_util.getScale(
				"locations.industry.initialIndustryDensity",
				modParams[""]["locations.industry.initialIndustryDensity"]
			)
	result.locationsConfig = locations

	result.economy = table_util.find(configDict, "economy")
	
	result.mapGenParam = mapGenParam

	return result
end

local record CreateNewMapParam
	record SaveGameConfig
		name : SaveGameInfo
		data : SaveGameData
	end
	saveGame : SaveGameConfig

	initialConfigDict : {{string, string}}
	activeMods : { string }

	map : boolean
	initial : integer
	autoActivateMods : boolean

	gameMap : GameMap
	mapPreviewInputs : MapPreviewInputs
end

local NewGameOrMapSettingsPage = react.RegisterRecipe("NewGameOrMapSettingsPage", function(createNewMapPageParams : MenuPageParam<CreateNewMapParam>) : TreeNodeId
	-- TPF3-MP begin: the added row picked is page state, so a pick redraws the page and its preview
	bigmapRefresh()
	bigmapRatioState = react.useState(bigmapRatio) as ReactStateT<integer>
	bigmapRatio = bigmapRatioState:old()
	bigmapPickState = react.useState(bigmapPick) as ReactStateT<integer>
	bigmapPick = bigmapPickState:old()
	if bigmapPick > #bigmapRows then
		bigmapPick = 0
	end
	-- TPF3-MP end
	local isMapEditor = createNewMapPageParams.params.map
	local newMapSelected = createNewMapPageParams.params.initialConfigDict ~= nil
	local strategy : SettingsStrategy
	if isMapEditor then
		if newMapSelected then
			strategy = mapEditorNewMapStrategy
		else
			strategy = mapEditorLoadMapStrategy
		end
	else
		if newMapSelected then
			strategy = newGameNewMapStrategy
		else
			strategy = newGameLoadMapStrategy
		end
	end

	if strategy.styleClasses ~= nil then
		react.setStyleClasses(strategy.styleClasses)
	end

	local saveGameData : SaveGameData
	local configDictState : ReactStateT<{{string, string}}>
	if newMapSelected then
		configDictState = react.useState(createNewMapPageParams.params.initialConfigDict)
	else
		saveGameData = createNewMapPageParams.params.saveGame.data
		configDictState = react.useState(saveGameData.info.configDict)
	end

	local possibleGenerators : {NewGameReactUtil.Generator} = {}
	local climateDesc : ClimateDesc = {}

	local mapParamsState : ReactStateT<NewGameReactUtil.NewGameState>
	local mapGenParamsState : ReactStateT<{string : integer}>
	local activeModsState : ReactStateT<{string}>
	local activeModsParamsState : ReactStateT<{string : {string : integer}}>
	if newMapSelected then
		local mapConfig = new_game_react_util.gatherMapConfig(configDictState:old())
		possibleGenerators = mapConfig.possibleGenerators
		climateDesc = mapConfig.climateDesc

		local useStates = new_game_react_util.prepareState(
			configDictState:old(),
			possibleGenerators,
			createNewMapPageParams.params.initial,
			createNewMapPageParams.params.autoActivateMods
		)
		mapParamsState = react.useState(useStates.mapParamsState)
		mapGenParamsState = react.useState(useStates.mapGenParamsState)
		activeModsState = react.useState(useStates.activeModsState)
		activeModsParamsState = react.useState(useStates.activeModsParamsState)
	else
		mapParamsState = react.useState(nil)
		mapGenParamsState = react.useState(nil)
		
		local useStates = new_game_react_util.prepareState(
			configDictState:old(),
			possibleGenerators,
			createNewMapPageParams.params.initial,
			createNewMapPageParams.params.autoActivateMods
		)
		activeModsState = react.useState(useStates.activeModsState)
		activeModsParamsState = react.useState(useStates.activeModsParamsState)
	end

	local mapPreviewReadyForInputRef : ReactRefT<MapPreviewInputs> = react.useRef(nil)

	local mapState : ReactStateT<GameMap> = react.useState()
	local emitTutorialErrorState : ReactStateT<boolean> = react.useState(false) -- wait until preview is done

	local startGameRef = react.useNodeRef()
	local function getCalloutApi() : CalloutApi
		return createNewMapPageParams.commonParams.calloutContainerRef:get():getApi()
	end
	react.useInputAction("IA_OPTION1", react.iaForward(startGameRef, "IA_SELECT"))

	local hintWidgetRef = react.useNodeRef(menu_icon_react_util.HintWidget)

	react.onUnmount(function() 
		getCalloutApi().close(startGameRef)

		if newMapSelected then
			local appConfig = api.type.AppConfig.new(api.util.getAppConfig())

			local t = table_util.copy(appConfig.mainMenuState) as table
			t["newGameState"] = mapParamsState:old()
			t["mapGenParamsState"] = mapGenParamsState:old()
			t["activeModsState"] = activeModsState:old()
			t["activeModsParamsState"] = activeModsParamsState:old()
			appConfig.mainMenuState = t

			api.util.setAppConfig(appConfig, false)
		end
	end)

	local runningIndex = 0
	local hintIdKey2itemIndex : {string : integer} = {}

	local registerHint = function(hintIdKey : string)
		if hintIdKey then
			runningIndex = runningIndex + 1
			hintIdKey2itemIndex[hintIdKey] = runningIndex
		end
	end

	local onHoverFocus = function(isSelected : boolean, hintIdKey : string)
		local itemId = hintIdKey2itemIndex[hintIdKey]
		local oldHovered = hintWidgetRef:get():getApi().getHovered()
		if isSelected and itemId and itemId ~= oldHovered then
			hintWidgetRef:get():getApi().setHovered(itemId)
		end
	end

	local settings = strategy.buildSettings and strategy.buildSettings(
		possibleGenerators,
		saveGameData,
		mapState,
		mapParamsState,
		mapGenParamsState,
		configDictState,
		activeModsParamsState
	) or {}

	local sChildren = {}
	for __, settingsAndTitle in ipairs(settings) do
		local settingsChildren : {TreeNodeId} = {}
		registerHint(settingsAndTitle.title)
		for __, v in ipairs(settingsAndTitle.settings) do
			registerHint(v.hintIdKey)
			table.insert(settingsChildren, builtin.Component{
				meta = {
					onAttention = function(hover : boolean)
						onHoverFocus(hover, v.hintIdKey)
					end,
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = {v.element},
				},
			})
		end
		table.insert(sChildren, content_card.ContentCard{
			title = settingsAndTitle.title,
			titleOnHoverFocus = function(isSelected : boolean)
				onHoverFocus(isSelected, settingsAndTitle.title)
			end,
			extraChildrenPermanent = settingsChildren,
		})
	end

	local modParams = newMapSelected and  activeModsParamsState:old() or saveGameData.info.modParams
	local baseModParams = modParams[""]

	local modIdList = savegame_react_util.makeModIdList(activeModsState:old())

	local yearValues = new_game_react_util.getYearValues()
	local initialTimeOfDayValues = new_game_react_util.getInitialTimeOfDayValues()
	local initialCloudCoverageValues = new_game_react_util.getInitialCloudCoverageValues()
	local startTimeIndex = baseModParams["gameTimeConfig.timeOfDayMode"]
	local startCloudCoverageIndex = baseModParams["weatherConfig.dynamicWeather"]
	local startBtn = menu_icon_react_util.makePrimaryButton(
		isMapEditor and _("Start Map Editor") or _("Start Game"),
		function()
			if newMapSelected then
				local map : GameMap = nil
				if isMapEditor then
					map = createMap(baseModParams, configDictState:old())
				else
					map = mapState:old()
				end

				local completedMods = new_game_react_util.autoActivateConfigDictMods(activeModsState:old(), configDictState:old())
				
				local completeModIdList = savegame_react_util.makeModIdList(completedMods)

				if not isMapEditor then
					app.setWaitForStartReadyGame()
				end
				app.startGame2(
					map,
					mapParamsState:old().seed,
					api.type.Date.new(yearValues[mapParamsState:old().startYearIndex], 1, 1),
					initialTimeOfDayValues[startTimeIndex],
					initialCloudCoverageValues[startCloudCoverageIndex],
					isMapEditor,
					completeModIdList,
					configDictState:old(),
					activeModsParamsState:old()
				)
				createNewMapPageParams.commonParams.setPage("ProgressPage", {})
			else
				modParams = activeModsParamsState:old()
				modIdList = savegame_react_util.makeModIdList(activeModsState:old())

				createNewMapPageParams.commonParams.setPage("ProgressPage", {
					onMount = function()
						local makeSaveId = function(mode : SaveGameInfo) : SaveGameId
							local saveId = api.type.SavegameId.new()
							saveId.path = mode.path
							saveId.saveGameName = mode.saveName
							saveId.saveGameNamespace = app.SaveGameNamespace.getSavemap()
							return saveId
						end
						if not isMapEditor then
							app.setWaitForStartReadyGame()
						end
									
						local infoCopy = api.type.SaveGameDetails.new(createNewMapPageParams.params.saveGame.data.info)
						infoCopy.modParams = modParams
						infoCopy.mods = modIdList
						
						app.loadGame(
							makeSaveId(createNewMapPageParams.params.saveGame.name),
							isMapEditor,
							infoCopy)
					end
				})
			end
		end,
		"loadSavegameButton, keyhint-builtin-right-inside, load-savegame-sound",
		(mapState:old() ~= nil or isMapEditor or (not newMapSelected)) and not emitTutorialErrorState:old(),
		nil,
		startGameRef
	)

	react.onStep(function()
		local currentGameMap : GameMap = mapState:old() or createNewMapPageParams.params.gameMap
		if mapPreviewReadyForInputRef:get() == nil or currentGameMap == nil then
			emitTutorialErrorState:set(false)
			return
		end
		local emitTutorialError = false
		local isTutorialActive = activeModsParamsState:old()[""]["guideSystemConfig.tutorial"] == 2
		if not isMapEditor and isTutorialActive then
			local isTutorialPossible = false
			local climateId : string = currentGameMap and currentGameMap.climateId or nil
			local defaultClimates : {string : boolean} = {
				["::/climates/dry/dry.clima"] = true,
				["::/climates/temperate/temperate.clima"] = true,
				["::/climates/subarctic/subarctic.clima"] = true,
				["::/climates/tropical/tropical.clima"] = true,
			}
			if defaultClimates[climateId] then
				local isBranchA = climateId == "::/climates/dry/dry.clima"
				local tag = isBranchA and "oil_platform" or "fishing_grounds"

				for __, industry in ipairs(currentGameMap.industries) do
					if industry.tag == tag then
						isTutorialPossible = true
					end
				end
			end
			emitTutorialError = not isTutorialPossible
		end
		emitTutorialErrorState:set(emitTutorialError)

		if emitTutorialError then
			local calloutParams = {
				recipe = callout_react_util.DefaultCalloutWrapper,
				recipeParam = {
					meta = {
						class = "info",
					},
					recipe = builtin.TextView,
					recipeParam = {
						text = _("Tutorial requires more water. Adjust water settings or change the seed."),
						meta = {
							class = "font-scale-body",
						},
					},
				},
				anchor = api.type.Vec2f.new(0.5, 1.0),
				gravity = api.type.Vec2f.new(0.5, 0.0),
				parent = startGameRef,
			} as CalloutApi.CalloutParamsT<IRecipeParamWithMeta>
			getCalloutApi().open(calloutParams)
		else
			getCalloutApi().close(startGameRef)
		end
	end)

	local childrenMain = {} -- expects one child on the left with 3/5 width and one on the right with 2/5 width

	table.insert(childrenMain, builtin.Component{
		meta = { class = "settingsContainer" },
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Vertical,
			children = {
				builtin.ScrollArea{
					horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
					verticalPolicy = builtin.type.ScrollBarPolicy.AsNeededButAlwaysReserveSpace,
					disableGamepadNavigation = true,
					content = builtin.Component{
						layout = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Vertical,
							children = sChildren,
						},
					},
				},
			},
		}
	})

	local makeGenericHint = function() : MenuIconReactUtil.HintWidgetParam.Data
		if newMapSelected then
			if isMapEditor then
				return {
					title = _("Create New Map"),
					description = _("Create a new empty map in a sandbox environment."),
				}
			end

			return {
				title = _("Create New Game"),
				description = _("Start a new game with a randomly generated map."),
			}
		end

		if isMapEditor then
			return {
				title = _("Edit Map"),
				description = _("Edit an existing map."),
			}
		end

		return {
			title = _("Play Map"),
			description = _("Start a new game with a predefined map."),
		}
	end

	local map : GameMap
	local mode : HeighmapGenerationMode
	if (not isMapEditor and newMapSelected) then
		map = app.makeEmptyMap(
			table_util.find(configDictState:old(), "climate") or "",
			getNumTiles(baseModParams["map.size"], baseModParams["map.format"]),
			0.0
		)
		mode = api.type.HeighmapGenerationMode.new()
		mode.terrainGenerator = mapParamsState:old().terrainGenerator
		mode.generateTowns = true
		mode.generateIndustries = true
		mode.generateAssets = true
	end

	local idData : {MenuIconReactUtil.HintWidgetParam.Data} = {}
	for __, settingsAndTitle in ipairs(settings) do
		table.insert(idData, {
			title = settingsAndTitle.title,
			description = settingsAndTitle.description,
		})
		for __, v in ipairs(settingsAndTitle.settings) do
			if v.hintIdKey then
				table.insert(idData, {
					title = v.title,
					description = v.description,
				})
			end
		end
	end

	local childrenRight = { }
	
	if not isMapEditor and newMapSelected then
		local initialMap = createNewMapPageParams.params.mapPreviewInputs and createNewMapPageParams.params.gameMap or nil
		local mapPreviewCompInput = newMapSelected and makeMapPreviewCompInputs(
			mapParamsState:old(),
			mapGenParamsState:old(),
			configDictState:old(),
			modParams
		) or nil
		if initialMap and not table_util.deepEquals(
			mapPreviewCompInput,
			createNewMapPageParams.params.mapPreviewInputs
		) then
			initialMap = nil
		end

		table.insert(childrenRight, builtin.MapPreviewComp{
			meta = { 
				class = "right-column-card, map-preview"
			}, 
			mapColoring = climateDesc.mapColoring,
			showMapSize = true,
			showTownAndIndustryCount = true,
			showBiomesColors = false,
			inputMap = map,
			mode = mode,
			economy = mapPreviewCompInput.economy,
			seed = mapPreviewCompInput.seed,
			locationsConfig = mapPreviewCompInput.locationsConfig,
			params = mapPreviewCompInput.mapGenParam,
			onStart = function()
				mapPreviewReadyForInputRef:set(nil)
			end,
			onReady = function(newMap : GameMap, _feedback : {string})
				log.verbose("Map is ready")
				mapState:set(newMap)
				mapPreviewReadyForInputRef:set(mapPreviewCompInput)
			end,
			initialMap = initialMap,
		})
	elseif not isMapEditor then
		local makeSaveId = function(info : SaveGameInfo) : SaveGameId
			local saveId = api.type.SavegameId.new()
			saveId.path = info.path
			saveId.saveGameName = info.saveName
			saveId.saveGameNamespace = not newMapSelected and app.SaveGameNamespace.getSavemap() or app.SaveGameNamespace.getSavegame()
			return saveId
		end
		childrenRight[#childrenRight + 1] = savegame_react_util.LoadPreview{
			meta = { class = "right-column-card, map-preview-picture" },
			saveGameData = createNewMapPageParams.params.saveGame.data,
			saveGameId = makeSaveId(createNewMapPageParams.params.saveGame.name),
		}
	end

	local difficultyComp : TreeNodeId = nil
	local noAchievementsComp : TreeNodeId = nil
	if not isMapEditor then
		difficultyComp = builtin.TextView {
			meta = {
				class = " map-meta-display, font-scale-title-3"
			},
			text = lang_util.format(_("Difficulty: {difficulty}"), {
				difficulty = difficulty_util.getLabel(
					difficulty_util.calculateDifficulty(newMapSelected and activeModsParamsState:old()[""] or saveGameData.info.modParams[""])
				),
			})
		}

		local modRep = app.getUserProfile():getModRep()
		local noAchievements = not modRep:couldAchievementsBeEarned(modIdList)

		if noAchievements then
			noAchievementsComp = builtin.TextView {
				meta = {
					class = "font-scale-body, warning-tape, disabled-achievements-warning"
				},
				text = getStringForPlatform(
					pGetText("platform-playstation", "Trophies cannot be earned."),
					pGetText("platform-xbox", "Achievements cannot be earned."),
					pGetText("platform-pc", "Achievements cannot be earned.")
				),
			}
		end
	end

	if isMapEditor then
		table.insert(childrenRight, menu_icon_react_util.HintImage{})
	end
	table.insert(childrenRight, menu_icon_react_util.HintWidget(react.ref(hintWidgetRef), {
		tabId2Hints = {[1] = idData},
		selectedTab = 1,
		placeHolderHint = makeGenericHint(),
	}))

	table.insert(childrenMain, builtin.Component{
		meta = { class = "right-two-fifths" },
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Vertical,
			children =  childrenRight,
		}
	})


	local tabState : ReactStateT<NewOrLoadPage> = react.useState("NewOrLoadMain")
	local tabWidgetChildren = {}

	table.insert(tabWidgetChildren, builtin.TabWidgetChild{
		indicator = builtin.TextView{
			meta = { class = "font-scale-tab-widget-indicator", },
			text = _("Settings"),
		},
		item = builtin.Component{
			meta = { class = "content", },
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = childrenMain,
			},
		},
		value = "NewOrLoadMain",
	})

	if not isMapEditor then
		local advancedParams = {
			activeModsParamsState = activeModsParamsState,
			configDictState = configDictState,
			defaultModParams = nil,
			defaultConfigDict = nil,
		}
		table.insert(tabWidgetChildren, builtin.TabWidgetChild{
			indicator = builtin.TextView{
				meta = { class = "font-scale-tab-widget-indicator", },
				text = _("Gameplay Settings"),
			},
			item = GameplaySettings({
				commonParams = createNewMapPageParams.commonParams,
				params = advancedParams,
			}),
			value = "NewOrLoadAdvanced",
		})
	end

	local modBack = function( -- back from modhub
		mods : {string},
		modParamsBack : {string : {string : integer}},
		initialConfigDict : {{string, string}}
	)
		if newMapSelected then
			local appConfig = api.type.AppConfig.new(api.util.getAppConfig())

			local t = table_util.copy(appConfig.mainMenuState) as table
			t["activeModsState"] = mods
			t["activeModsParamsState"] = modParamsBack
			appConfig.mainMenuState = t

			api.util.setAppConfig(appConfig, false)
			createNewMapPageParams.commonParams.setPage("NewGameOrMapSettings", {
				initialConfigDict = initialConfigDict,
				map = isMapEditor,
				initial = false,
			})
		else
			local copy = api.type.SaveGameData.new(createNewMapPageParams.params.saveGame.data)
			local infoCopy = api.type.SaveGameDetails.new(copy.info)
			infoCopy.mods = savegame_react_util.makeModIdList(mods)
			infoCopy.modParams = modParamsBack
			copy.info = infoCopy

			createNewMapPageParams.commonParams.setPage("NewGameOrMapSettings", {
				saveGame = {
					name = createNewMapPageParams.params.saveGame.name,
					data = copy
				},
				map = isMapEditor,
				initial = false,
			})
		end

		tabState:set("NewOrLoadMods")
	end

	local textModTab = #activeModsState:old() > 0 and lang_util.format(_("Mods ({numMods})"), {
		numMods = lang_util.formatInt(#activeModsState:old())
	}) or _("Add Mods")

	local modSelectorParams = {
		activeModsState = activeModsState,
		activeModsParamsState = activeModsParamsState,
		configDictState = configDictState,
		back = modBack,
	}
	table.insert(tabWidgetChildren, builtin.TabWidgetChild{
		indicator = builtin.TextView{
			meta = { class = "font-scale-tab-widget-indicator", },
			text = textModTab,
		},
		item = ModSelectorPage({
			commonParams = createNewMapPageParams.commonParams,
			params = modSelectorParams,
		}),
		value = "NewOrLoadMods",
	})

	local tabWidgetRef = react.useNodeRef()
	react.useInputAction("IA_TABRIGHT", react.iaForward(tabWidgetRef))
	react.useInputAction("IA_TABLEFT", react.iaForward(tabWidgetRef))
	react.useInputAction("IA_OPTION1", react.iaForward(startGameRef, "ACTION_CLICK"))
	return menu_icon_react_util.makePage(
		createNewMapPageParams.commonParams,
		strategy.getTitle(),
		function()
			createNewMapPageParams.commonParams.setPage("NewGame", { map = isMapEditor })
		end,
		menu_icon_react_util.makeMainOuterCard(
			true, -- addSpacer
			{
				builtin.FloatingLayout{
					children = {
						builtin.FloatingLayoutChild{
							h = -1,
							v = -1,
							item = builtin.BoxLayout{
								orientation = builtin.type.Orientation.Vertical,
								children = {
									builtin.TabWidget(react.ref(tabWidgetRef), {
										orientation = builtin.type.TabOrientation.North,
										deselectAllowed = false,
										showIndicators = true,
										value = tabState:old(),
										tabs = tabWidgetChildren,
										onValueChange = function(newTab : NewOrLoadPage)
											tabState:set(newTab)
										end
									}),
									builtin.Component{
										meta = {
											class = "start-button-comp",
										},
										layout = builtin.BoxLayout{
											orientation = builtin.type.Orientation.Horizontal,
											children = {
												startBtn,
											},
										},
									},
								},
							},
						},
						builtin.FloatingLayoutChild{
							h = 0.0,
							v = 1.0,
							item = builtin.Component{
								layout = builtin.BoxLayout{
									orientation = builtin.type.Orientation.Horizontal,
									children = {
										difficultyComp,
										noAchievementsComp,
									},
								},
							}
						},
					},
				},
			}
		),
		{}
	)
end)

return NewGameOrMapSettingsPage
