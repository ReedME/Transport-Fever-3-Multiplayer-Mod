//! Big Maps' Lua in the game's Lua states (from Big Maps' branch, where
//! the hook's own menu patch carried it, `menu_entry.rs` on
//! `feat/bigmaps-40420`): run after the hook's own, it wraps
//! `resolveutil.loadfile` once more, so that
//!
//! - `gui/menu/new_game_or_map_settings_page.tl` is served from the
//!   package's game mod, `tpf3mp_bigmap_1` (the size rows);
//! - `base/difficulty_util.tl` is wrapped as it loads, so that `getScale`
//!   answers Big Maps' added density levels (`menu.extendDifficulty` in
//!   the game mod's `menu.lua`, with its `ladder.lua`).
//!
//! It also leaves the machine's memory (`__tpf3mp_ram_mb`) and whether the
//! page holds its sizes against it (`__tpf3mp_memory_gate`) in
//! `resolveutil`, where `menu.lua` reads them. A copy that does not load
//! falls back to the game's file; a path already in the game mod is never
//! redirected, so the wrap cannot loop.

/// The chunk, without its first lines ([`chunk`]).
const WRAP: &str = r#"
local ru = resolveutil
if type(ru) ~= "table" then return end
if ru.__tpf3mp_bigmap then return end
ru.__tpf3mp_bigmap = true
ru.__tpf3mp_ram_mb = __tpf3mp_ram_mb
ru.__tpf3mp_memory_gate = __tpf3mp_memory_gate
local PAGE = "tpf3mp_bigmap_1::/gui/menu/new_game_or_map_settings_page.tl"
local orig = ru.loadfile
ru.loadfile = function(path, ...)
	if type(path) == "string" and not path:find("^tpf3mp_bigmap_1::") then
		if path:find("gui/menu/new_game_or_map_settings_page%.tl$") then
			local ok, chunk, err = pcall(orig, PAGE, ...)
			if ok and chunk then
				pcall(debugPrint, "[tpf3mp] big maps: " .. path .. " is served from " .. PAGE)
				return chunk, err
			end
			pcall(debugPrint, "[tpf3mp] big maps: " .. PAGE .. " is not loadable (" .. tostring(ok and err or chunk) .. "); the page stays the game's")
		elseif path:find("difficulty_util%.tl$") then
			local chunk, err = orig(path, ...)
			if not chunk then return chunk, err end
			return function(...)
				local du = chunk(...)
				local ok, why = pcall(function()
					local menuChunk = orig("tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/menu.lua")
					local ladderChunk = orig("tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/ladder.lua")
					if not menuChunk or not ladderChunk then error("the mod tpf3mp_bigmap_1 is not here", 0) end
					local menu, ladder = menuChunk(), ladderChunk()
					local note = function(text) pcall(debugPrint, "[tpf3mp] big maps: density " .. text) end
					if menu.extendDifficulty(du, ladder, note) then
						note("levels " .. (menu.STOCK_DENSITY_LEVELS + 1) .. " to " .. (menu.STOCK_DENSITY_LEVELS + #ladder) .. " are live")
					end
				end)
				if not ok then pcall(debugPrint, "[tpf3mp] big maps: the density levels are the game's own (" .. tostring(why) .. ")") end
				return du
			end, err
		end
	end
	return orig(path, ...)
end
pcall(debugPrint, "[tpf3mp] big maps: resolveutil.loadfile is wrapped")
"#;

/// The chunk, with the machine's memory in MB, if known, and whether the
/// page holds its sizes against it, in front.
pub fn chunk(ram_mb: Option<u64>, memory_gate: bool) -> String {
    let ram = ram_mb.map_or_else(|| "nil".to_owned(), |mb| mb.to_string());
    format!("local __tpf3mp_ram_mb = {ram}\nlocal __tpf3mp_memory_gate = {memory_gate}\n{WRAP}")
}

/// The machine's physical memory in MB (MiB), if Windows says.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn physical_memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; dwLength is set as the call
    // requires, and the struct lives across the call.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a valid, initialised out-parameter.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.ullTotalPhys > 0).then_some(status.ullTotalPhys / (1024 * 1024))
}

/// Elsewhere the memory is not known: the page offers what the walls allow.
#[cfg(not(windows))]
pub fn physical_memory_mb() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "tpf3mp_bigmap_1::/gui/menu/new_game_or_map_settings_page.tl";

    /// A Lua whose loader records what it was asked for and has no file
    /// for the paths in `missing`, with the hook's own wrap and then the
    /// chunk run in it, twice.
    fn wrapped(ram_mb: Option<u64>, missing: &[&str]) -> mlua::Lua {
        let lua = mlua::Lua::new();
        let missing = lua.create_sequence_from(missing.iter().copied()).unwrap();
        lua.globals().set("missing", missing).unwrap();
        lua.load(
            r#"
            debugPrint = function() end
            asked = {}
            resolveutil = { loadfile = function(path)
                asked[#asked + 1] = path
                for _, m in ipairs(missing) do
                    if m == path then return nil, "no such file" end
                end
                return function() return path end
            end }
        "#,
        )
        .exec()
        .unwrap();
        lua.load(chunk(ram_mb, true)).exec().unwrap();
        lua.load(chunk(ram_mb, true)).exec().unwrap();
        lua
    }

    fn load_through(lua: &mlua::Lua, path: &str) -> (String, Vec<String>) {
        lua.load("asked = {}").exec().unwrap();
        let chunk: mlua::Function = lua
            .load(format!("return resolveutil.loadfile({path:?})"))
            .eval()
            .unwrap();
        let served: String = chunk.call(()).unwrap();
        let asked: mlua::Table = lua.globals().get("asked").unwrap();
        (
            served,
            asked.sequence_values().map(Result::unwrap).collect(),
        )
    }

    #[test]
    fn the_new_game_page_comes_from_the_game_mod() {
        let lua = wrapped(Some(32_768), &[]);
        assert_eq!(
            load_through(&lua, "::/gui/menu/new_game_or_map_settings_page.tl"),
            (PAGE.to_owned(), vec![PAGE.to_owned()]),
            "wrapped once, though run twice"
        );
        for path in [PAGE, "::/gui/menu/main_page.tl"] {
            assert_eq!(
                load_through(&lua, path),
                (path.to_owned(), vec![path.to_owned()])
            );
        }
        let ram: u64 = lua
            .load("return resolveutil.__tpf3mp_ram_mb")
            .eval()
            .unwrap();
        assert_eq!(ram, 32_768);
    }

    #[test]
    fn without_the_game_mod_the_page_is_the_games() {
        let lua = wrapped(None, &[PAGE]);
        let game = "::/gui/menu/new_game_or_map_settings_page.tl";
        assert_eq!(
            load_through(&lua, game),
            (game.to_owned(), vec![PAGE.to_owned(), game.to_owned()])
        );
    }

    #[test]
    fn the_density_levels_come_from_the_game_mods_ladder() {
        let lua = mlua::Lua::new();
        lua.globals()
            .set(
                "menu_src",
                include_str!(
                    "../package/mod/tpf3mp_bigmap_1/content/scripts/tpf3mp_bigmap/menu.lua"
                ),
            )
            .unwrap();
        lua.globals()
            .set(
                "ladder_src",
                include_str!(
                    "../package/mod/tpf3mp_bigmap_1/content/scripts/tpf3mp_bigmap/ladder.lua"
                ),
            )
            .unwrap();
        lua.load(
            r#"
            debugPrint = function() end
            resolveutil = { loadfile = function(path)
                if path == "tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/menu.lua" then return loadstring(menu_src) end
                if path == "tpf3mp_bigmap_1::/scripts/tpf3mp_bigmap/ladder.lua" then return loadstring(ladder_src) end
                if path:find("difficulty_util%.tl$") then
                    return function()
                        local scales = { 0.25, 0.5, 1, 2, 4 }
                        return { getScale = function(param, level) return scales[level] or 1.0 end }
                    end
                end
                return nil, "no such file"
            end }
        "#,
        )
        .exec()
        .unwrap();
        lua.load(chunk(None, true)).exec().unwrap();
        let (stock, added): (f64, f64) = lua
            .load(
                r#"
                local du = resolveutil.loadfile("::/base/difficulty_util.tl")()
                return du.getScale("locations.towns.frequency", 3),
                    du.getScale("locations.towns.frequency", 6)
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(stock, 1.0, "the game's own levels are the game's");
        assert!(
            (added - 0.7656).abs() < 1e-9,
            "level 6 is the ladder's first row, got {added}"
        );
    }
}
