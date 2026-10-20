-- The New Game page's side of big maps: which rows to add to the size
-- dropdown, and the tiles a picked row and ratio stand for.
--
-- On TPF2 this was native: silver2127's Big Maps (tpf2-bigmap) appended
-- rows to the size dropdown and answered UI::GetNumTilesNew for them,
-- leaving every stock row to the game. TF3's New Game page is a script, so
-- the mod's copy of it (gui/menu/new_game_or_map_settings_page.tl, served
-- by the TPF3-MP hook in place of the game's) does the same with these
-- functions. This file uses no game API apart from reading what the hook
-- left in `resolveutil`: it is plain Lua, tested from
-- crates/tpf3mp-bigmap/tests/mod_lua.rs.
--
-- The rows come from ladder.lua, which is generated from the big-map
-- settings, so the menu offers exactly the sizes those settings can build.

local menu = {}

-- The labels to append to the size dropdown, after the game's own rows.
function menu.labels(ladder)
	local labels = {}
	for i, row in ipairs(ladder) do
		labels[i] = row.label
	end
	return labels
end

-- The tiles for the dropdowns' picks: `sizeIndex` and `ratioIndex` count
-- from 0, as the dropdowns do, and the game's own `stockRows` rows come
-- first. Returns:
--   nil                  a stock row: the game answers, unchanged;
--   x, y                 an added row's shape, in tiles;
--   false, reason        an added row whose shape the settings cannot
--                        build, or an index past the added rows.
function menu.tiles(ladder, stockRows, sizeIndex, ratioIndex)
	if sizeIndex < stockRows then
		return nil
	end
	local row = ladder[sizeIndex - stockRows + 1]
	if row == nil then
		return false, "no added size row " .. tostring(sizeIndex)
	end
	local shape = row.shapes[ratioIndex + 1]
	if shape == nil then
		return false, "no ratio " .. tostring(ratioIndex)
	end
	if not shape then
		return false, row.label .. " cannot be built at this ratio with these settings"
	end
	return shape[1], shape[2]
end

-- The machine's memory in MB, as the TPF3-MP hook found it, or nil; with
-- the memory check turned off (TPF3MP_BIGMAP_MEMORY_GATE=0, which the hook
-- passes as __tpf3mp_memory_gate = false), unlimited: every size and ratio
-- the walls allow is offered, whether it fits or not.
function menu.ramMb()
	local ok, ru = pcall(function()
		return resolveutil
	end)
	if ok and type(ru) == "table" and ru.__tpf3mp_memory_gate == false then
		return math.huge
	end
	if ok and type(ru) == "table" and type(ru.__tpf3mp_ram_mb) == "number" and ru.__tpf3mp_ram_mb > 0 then
		return ru.__tpf3mp_ram_mb
	end
	return nil
end

-- The rows of `ladder` this machine has the memory to generate, and at
-- most one short line saying which are left out and why: the page shows it
-- in the narrow settings column, on one line. With the memory not known,
-- none: a size that may not fit is not offered on a guess.
function menu.offered(ladder, ramMb)
	local rows, notes = {}, {}
	if type(ramMb) ~= "number" or ramMb <= 0 then
		notes[1] = "Bigger sizes hidden: memory unknown"
		return rows, notes
	end
	if ramMb == math.huge then
		notes[1] = "Memory check off"
	end
	local hidden = {}
	for _, row in ipairs(ladder) do
		if row.peakMb <= ramMb then
			rows[#rows + 1] = row
		else
			hidden[#hidden + 1] = row
		end
	end
	local have = math.floor(ramMb / 1024)
	if #hidden == 1 then
		notes[#notes + 1] = string.format(
			"%s hidden: needs %d GB, have %d GB",
			hidden[1].label,
			math.ceil(hidden[1].peakMb / 1024),
			have
		)
	elseif #hidden > 1 then
		local least = hidden[1].peakMb
		for _, row in ipairs(hidden) do
			least = math.min(least, row.peakMb)
		end
		notes[#notes + 1] = string.format(
			"%d sizes hidden: need %d+ GB, have %d GB",
			#hidden,
			math.ceil(least / 1024),
			have
		)
	end
	return rows, notes
end

-- The size dropdown's entries: the game's own, then the added rows. The
-- game's lists may be native vectors, not tables: they are read by index
-- and length only, as the game's own scripts read them.
function menu.sizeValues(stockValues, rows)
	local values = {}
	for i = 1, #stockValues do
		values[i] = stockValues[i]
	end
	for _, label in ipairs(menu.labels(rows)) do
		values[#values + 1] = label
	end
	return values
end

-- A parameter's numbers, or nil when it has none (the game treats an empty
-- list as none). A native vector counts as well as a table.
local function numbersOf(stockNumbers)
	if stockNumbers == nil then
		return nil
	end
	local ok, count = pcall(function()
		return #stockNumbers
	end)
	if ok and type(count) == "number" and count > 0 then
		return stockNumbers
	end
	return nil
end

-- The dropdown's entry (from 1) for the game's "map.size" `value` and the
-- added row picked (0 for none).
function menu.indexOf(value, pick, stockCount, stockNumbers, rowCount)
	if pick >= 1 and pick <= rowCount then
		return stockCount + pick
	end
	local numbers = numbersOf(stockNumbers)
	if numbers then
		local best, distance = 1, math.abs(numbers[1] - value)
		for i = 2, #numbers do
			if math.abs(numbers[i] - value) < distance then
				best, distance = i, math.abs(numbers[i] - value)
			end
		end
		return best
	end
	return math.max(1, math.min(stockCount, math.floor(value)))
end

-- What picking the dropdown's entry `index` (from 1) sets: the game's
-- "map.size" value and the added row picked (0 for one of the game's own).
-- An added row sets "map.size" to the game's largest size, a value the game
-- knows, which stays what the save says.
function menu.choose(index, stockCount, stockNumbers)
	local numbers = numbersOf(stockNumbers)
	if index > stockCount then
		return numbers and numbers[#numbers] or stockCount, index - stockCount
	end
	return numbers and numbers[index] or index, 0
end

-- The tiles of the added row `pick` (from 1) at the ratio `ratioIndex`
-- (from 0), the short side first as the game's own shapes have it; or
-- nil and the reason.
function menu.gameTiles(rows, pick, ratioIndex)
	local x, y = menu.tiles(rows, 0, pick - 1, ratioIndex)
	if not x then
		return nil, y
	end
	return math.min(x, y), math.max(x, y)
end

-- Longer ratios. The ratio dropdown gets 1:6 to 1:10 after the game's own
-- 1:1 to 1:5: what makes long edges reachable. Each keeps about the 1:1
-- square's area (ladder.lua has the shapes, false where the settings
-- cannot build one). The game's "map.format" stays its own last ratio,
-- 1:5, so the save and every other reader see a value the game knows; the
-- added ratio picked is the page's (1 is 1:6), as the added row picked is.
menu.STOCK_RATIOS = 5
menu.EXTRA_RATIOS = { 6, 7, 8, 9, 10 }

-- The added ratios offered for one size: `shapes` its ten shapes (an added
-- row's, or a stock entry's five added ones placed after five nils),
-- `peakMb` the largest one's expected peak, `label` its name. Returns the
-- picks offered (1 is 1:6), and a line saying why any other is missing.
function menu.extraRatios(shapes, peakMb, ramMb, label)
	local offered, missing = {}, {}
	if type(shapes) ~= "table" then
		return offered, nil
	end
	for i, k in ipairs(menu.EXTRA_RATIOS) do
		if shapes[menu.STOCK_RATIOS + i] then
			offered[#offered + 1] = i
		else
			missing[#missing + 1] = "1:" .. k
		end
	end
	if #offered > 0 and (type(ramMb) ~= "number" or ramMb <= 0) then
		return {}, "Longer ratios hidden: memory unknown"
	end
	if #offered > 0 and type(peakMb) == "number" and peakMb > ramMb then
		return {}, string.format(
			"Longer ratios hidden: need %d GB, have %d GB",
			math.ceil(peakMb / 1024),
			math.floor(ramMb / 1024)
		)
	end
	if #missing == 0 then
		return offered, nil
	end
	if #offered == 0 then
		return offered, "No longer ratios for " .. tostring(label)
	end
	return offered, string.format(
		"%s: too long for %s",
		table.concat(missing, ", "),
		label
	)
end

-- The ten shapes of the game's own size whose 1:1 square is `squareTiles`
-- (the five added ones after five nils), its peak, and its label; or nil.
function menu.stockShapes(ladder, squareTiles)
	local stock = type(ladder) == "table" and ladder.stock or nil
	local entry = stock and stock[squareTiles] or nil
	if entry == nil then
		return nil
	end
	local shapes = {}
	for i, shape in ipairs(entry.shapes) do
		shapes[menu.STOCK_RATIOS + i] = shape
	end
	return shapes, entry.peakMb, string.format("%d x %d tiles", squareTiles, squareTiles)
end

-- The ratio dropdown's entries: the game's own, then the added ratios
-- offered.
function menu.formatValues(stockValues, offered)
	local values = {}
	for i = 1, #stockValues do
		values[i] = stockValues[i]
	end
	for _, pick in ipairs(offered) do
		values[#values + 1] = "1:" .. menu.EXTRA_RATIOS[pick]
	end
	return values
end

-- The dropdown's entry (from 1) for the game's "map.format" `value` and the
-- added ratio picked (0 for none).
function menu.formatIndexOf(value, ratioPick, stockCount, stockNumbers, offered)
	for position, pick in ipairs(offered) do
		if pick == ratioPick then
			return stockCount + position
		end
	end
	return menu.indexOf(value, 0, stockCount, stockNumbers, 0)
end

-- What picking the ratio dropdown's entry `index` (from 1) sets: the game's
-- "map.format" value (its last ratio for an added one) and the added ratio
-- picked (0 for one of the game's own).
function menu.formatChoose(index, stockCount, stockNumbers, offered)
	if index > stockCount and offered[index - stockCount] ~= nil then
		local value = menu.choose(stockCount, stockCount, stockNumbers)
		return value, offered[index - stockCount]
	end
	local value = menu.choose(math.min(index, stockCount), stockCount, stockNumbers)
	return value, 0
end

-- The tiles of `shapes` at the added ratio `ratioPick` (from 1), the short
-- side first; or nil and the reason.
function menu.extraTiles(shapes, ratioPick)
	local shape = type(shapes) == "table" and shapes[menu.STOCK_RATIOS + ratioPick] or nil
	if not shape then
		return nil, "no shape at 1:" .. tostring(menu.EXTRA_RATIOS[ratioPick])
	end
	return math.min(shape[1], shape[2]), math.max(shape[1], shape[2])
end

-- Density levels. Town and industry counts are a density per km², so a big
-- map at the stock sliders has many more towns and industries than any
-- stock map. The Town Density and Industry Density sliders get one more
-- level per ladder row, before the game's own five: "-" to "-...-", one dash per
-- row (the Gigantomaniac count at that size): the stock Medium scaled by the row's densityScale, which gives
-- that row's square the counts stock Gigantomaniac 1:1 has at Medium.
-- Every row counts, offered on this machine or not, so a level is the same
-- number in every game. As after silver2127's Big Maps for TPF2
-- (tpf2-bigmap), whose added density levels these follow.
--
-- The game turns a slider's level (from 1) into a factor with
-- difficulty_util.getScale, in the New Game page's preview (which the new
-- world is generated from) and in the base mod's run script on every load
-- (the runtime industry target). The TPF3-MP hook wraps that module with
-- menu.extendDifficulty, so both read the added levels; without the hook a
-- level past the game's own reads as the game's fallback, 1.0.

-- The game's own levels for each density parameter, and its Medium.
menu.DENSITY_PARAMS = {
	["locations.towns.frequency"] = true,
	["locations.industry.initialIndustryDensity"] = true,
	["locations.industry.targetIndustryDensity"] = true,
}
menu.STOCK_DENSITY_LEVELS = 5
menu.STOCK_DENSITY_MEDIUM = 3

-- The density slider's labels and the level each stores, sparsest first:
-- one per ladder row, the largest size first (its scale is the smallest),
-- then the game's own five. The levels keep their numbers (the game's
-- 1 to 5, the rows' 6 on), so the slider's order changes nothing saved.
function menu.densityChoices(stockValues, ladder)
	local values, numbers = {}, {}
	for i = #ladder, 1, -1 do
		-- "----" for the last row (sparsest) down to "-" for the first:
		-- as short as the slider's own labels.
		values[#values + 1] = string.rep("-", i)
		numbers[#numbers + 1] = menu.STOCK_DENSITY_LEVELS + i
	end
	for i = 1, #stockValues do
		values[#values + 1] = stockValues[i]
		numbers[#numbers + 1] = i
	end
	return values, numbers
end

-- The factor for `param` at `level` (from 1) given the game's own
-- getScale, or nil when the level is one of the game's own (or `param` is
-- not a density) and the game answers.
function menu.densityScale(ladder, stockGetScale, param, level)
	if not menu.DENSITY_PARAMS[param] or type(level) ~= "number" or level <= menu.STOCK_DENSITY_LEVELS then
		return nil
	end
	local row = ladder[math.floor(level) - menu.STOCK_DENSITY_LEVELS]
	if row == nil or type(row.densityScale) ~= "number" then
		return nil
	end
	return stockGetScale(param, menu.STOCK_DENSITY_MEDIUM) * row.densityScale
end

-- Wraps difficulty_util (`du`) so getScale answers the added levels;
-- `note(text)` is told the first time each answer is given. Returns true
-- once wrapped (never twice).
function menu.extendDifficulty(du, ladder, note)
	if type(du) ~= "table" or type(du.getScale) ~= "function" or du.__tpf3mp_density then
		return false
	end
	local stock = du.getScale
	local told = {}
	du.getScale = function(param, level, ...)
		local scale = menu.densityScale(ladder, stock, param, level)
		if scale == nil then
			return stock(param, level, ...)
		end
		local key = tostring(param) .. "@" .. tostring(level)
		if note and not told[key] then
			told[key] = true
			note(string.format("%s level %d is %.4f", param, level, scale))
		end
		return scale
	end
	du.__tpf3mp_density = true
	return true
end

return menu
