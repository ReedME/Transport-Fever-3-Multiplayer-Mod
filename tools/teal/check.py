"""Type-check Teal files the mod ships against the game's own Teal and type
declarations, as the game compiles them.

    python -I tools/teal/check.py <game folder> <file.tl> [<compare.tl>]

The game ships its compiler (`base/tl.lua`, inside `base/content/base.zip`,
Teal 0.24.1 with Urban Games' patches) and its declarations
(`api/tealdef`, `base/tealdef`: global types, which name one another in
cycles). This checks every declaration as one file, in the same
environment until the errors stop falling (a global type declared on one
pass is known on the next), then type-checks the file. With a second file
(the game's original of a file the mod patches) it prints only the errors
the first has that the second does not: those the patch brought in.

Needs Python's `lupa` (an embedded Lua 5.4). Reads the game's files; writes
nothing. Exits 1 when the file has errors (new ones, when comparing).
"""

import io
import os
import re
import sys
import zipfile

import lupa.lua54 as lupa


def game_tl(game):
    """The game's tl.lua, from base.zip (local headers say UG, not PK)."""
    data = open(os.path.join(game, "base", "content", "base.zip"), "rb").read()
    for ug, pk in ((b"UG\x03\x04", b"PK\x03\x04"), (b"UG\x01\x02", b"PK\x01\x02"), (b"UG\x05\x06", b"PK\x05\x06")):
        data = data.replace(ug, pk)
    source = zipfile.ZipFile(io.BytesIO(data)).read("base/tl.lua").decode("utf-8")
    # Its unused-variable warning trips over a type alias declared without a
    # place (the game's own declarations have them); the warning is not a
    # check, so it is skipped there.
    crash = "         var.aliasing.declared_at.elide_type = false"
    if crash in source:
        source = source.replace(crash, "         if var.aliasing.declared_at then var.aliasing.declared_at.elide_type = false end")
    return source


def declarations(game):
    """Every declaration file as one, the API's first, without the lines
    that load one another (`require "api.util"`) or set `recordDef`."""
    parts = []
    for root in ("api/tealdef", "base/tealdef"):
        top = os.path.join(game, root)
        for folder, _dirs, files in sorted(os.walk(top)):
            for name in sorted(files):
                if name.endswith(".d.tl") and name != "api_def.d.tl":
                    text = open(os.path.join(folder, name), encoding="utf-8").read()
                    text = re.sub(r'(?m)^\s*recordDef\s*=\s*true\s*$', "", text)
                    text = re.sub(r'(?m)^\s*require\s+"[^"]+"\s*$', "", text)
                    parts.append(f"-- {root}/{os.path.relpath(os.path.join(folder, name), top)}\n{text}")
    return "\n".join(parts)


DRIVER = r"""
local tl_source, defs = ...
log = { error = function() end, warning = function() end, message = function() end }
local tl = assert(load(tl_source, "@tl.lua"))()
local env = assert(tl.new_env({}))
tl.check_string("global recordDef : boolean", env, "@recordDef")
local last, errors = math.huge, 0
for pass = 1, 8 do
    env.loaded = {}
    local result = tl.check_string(defs, env, "tealdef" .. pass .. ".d.tl")
    errors = #(result.syntax_errors or {}) + #(result.type_errors or {})
    if errors >= last then break end
    last = errors
end
return env, errors
"""

CHECK = r"""
local tl_source, env, name, text = ...
local tl = assert(load(tl_source, "@tl.lua"))()
env.loaded = {}
local result = tl.check_string(text, env, name)
local out = {}
for _, kind in ipairs({ "syntax_errors", "type_errors" }) do
    for _, e in ipairs(result[kind] or {}) do
        out[#out + 1] = { e.y or 0, e.x or 0, tostring(e.msg) }
    end
end
return out
"""


def errors(run_check, tl_source, env, name, text):
    found = run_check(tl_source, env, name, text)
    return [(int(e[1]), int(e[2]), str(e[3])) for e in found.values()]


def main():
    if len(sys.argv) not in (3, 4):
        print(__doc__)
        return 2
    game, path = sys.argv[1], sys.argv[2]
    compare = sys.argv[3] if len(sys.argv) == 4 else None
    lua = lupa.LuaRuntime(unpack_returned_tuples=True)
    tl_source = game_tl(game)
    env, left = lua.eval("function(...) " + DRIVER + " end")(tl_source, declarations(game))
    print(f"the game's declarations loaded, {left} error(s) left in them")
    run_check = lua.eval("function(...) " + CHECK + " end")
    text = open(path, encoding="utf-8").read()
    mine = errors(run_check, tl_source, env, os.path.basename(path), text)
    if compare:
        original = open(compare, encoding="utf-8").read()
        theirs = errors(run_check, tl_source, env, os.path.basename(compare), original)
        # An error is the game's own when the same message is reported on
        # the same source line there (lines move: the patch adds some).
        def line(src, y):
            lines = src.splitlines()
            return lines[y - 1].strip() if 0 < y <= len(lines) else ""
        known = {(msg, line(original, y)) for y, _x, msg in theirs}
        new = [e for e in mine if (e[2], line(text, e[0])) not in known]
        print(f"{path}: {len(mine)} error(s), {len(theirs)} in the game's own; {len(new)} new")
        for y, x, msg in new:
            print(f"  {y}:{x}: {msg}")
        return 1 if new else 0
    print(f"{path}: {len(mine)} error(s)")
    for y, x, msg in mine:
        print(f"  {y}:{x}: {msg}")
    return 1 if mine else 0


if __name__ == "__main__":
    sys.exit(main())
