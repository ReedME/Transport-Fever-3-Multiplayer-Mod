-- tpf3mp/bridge.lua -- the Lua half of the link between the mod and the hook.
--
-- The hook (crates/tpf3mp-hook) runs only in a game the TPF3-MP launcher
-- started (DECISIONS.md, D11). There it gives every Lua state that calls
-- `print` one global table, so the mod prints before it looks for it:
--
--   tpf3mp_native = {
--     version = 15,                 -- bridge.VERSION; anything else is refused
--     command = function(action, password), -- the player acted: an action
--                                   -- table, for the room to order, and a
--                                   -- company's password for joining or
--                                   -- locking it, which the room seals
--                                   -- -> true, ticket | false, why
--     take    = function(),         -- marks a simulation update begun;
--                                   -- nil actions in runtime update batches
--     takeReplay = function(token), -- ordered actions, origins and seals,
--                                   -- once, in the engine's handleEvent
--     replayed = function(token, ok, why), -- done after action reports and
--                                   -- script-state persistence
--     log     = function(line),     -- a line for hook.log
--     poll    = function(),         -- in the GUI, every frame: what the hook
--                                   -- asks, { replay = token }, { save = name }
--                                   -- or { load = name }
--                                   -- (the game's own save folder), or nil
--     saved   = function(name, ok, why), -- the GUI's answer to a save
--     world   = function(),         -- a world's GUI started
--     room    = function(),         -- whether the room's game runs -> boolean
--     checkpoint = function(),      -- report due, absolute simulation step
--     scanned = function(ok, why, ms), -- rolling read completed; failure holds
--     lanes   = function(t),        -- those lanes, { [lane] = text }
--                                   -- -> true | false, why
--     clicks  = function(),         -- in the GUI: the player's builds queued
--                                   -- in the room's game so far, or nil where
--                                   -- the hook cannot take them to the room
--     built   = function(n),        -- optional; in the GUI: the build the
--                                   -- module editor or a terrain tool queued
--                                   -- at click n, as the hook read it | nil,
--                                   -- why | nil (neither's)
--     replaying = function(on),     -- the game script applies the room's
--                                   -- actions (true) or is done (false)
--     terrain = function(t),        -- optional; while the room's actions run:
--                                   -- arms the next build sent with the
--                                   -- terraform grid t -> true | nil, why;
--                                   -- terrain() disarms -> whether a build
--                                   -- was filled | nil (none armed)
--     applied = function(i, ok, entity, why), -- in a game script's postUpdate:
--                                   -- what became of the batch's action i
--     results = function(),         -- in the GUI: what became of the player's
--                                   -- own actions since the last call,
--                                   -- { { ticket =, ok =, entity =, why = } }
--     status  = function(),         -- the room, for the Multiplayer window, or
--                                   -- nil before its game: { room = (display), room_id = (stable), speed =,
--                                   -- diverged =, me_id =, players = { {
--                                   -- name =, connected =, owner =, me =,
--                                   -- id = } } }
--     chat    = function(),         -- what the room's members said since the
--                                   -- last call, { { from =, text = } }
--     say     = function(text),     -- says text to the room -> true |
--                                   -- false, why
--     dump    = function(),         -- optional; in a game script's
--                                   -- postUpdate at a checkpoint: the lanes
--                                   -- to dump, { step =, lanes = { n, ... } },
--                                   -- once, or nil
--     dumped  = function(lane, entry), -- optional; one entry of a lane
--                                   -- dumped, for hook.log -> true | false
--                                   -- (no more taken)
--     mods    = function(list),     -- optional; the mods to load a save
--                                   -- whose mods are `list` (names, one a
--                                   -- line) with -> list, left out, added
--                                   -- (the same way) | nil without the
--                                   -- room's lists; mods() alone -> true
--                                   -- | nil: whether the room gave them
--     personal = function(),        -- optional; this player's personal
--                                   -- mods (names, one a line) | nil
--     shared  = function(),         -- optional; the room's shared mods
--                                   -- (names, one a line) | nil
--     note    = function(key, value), -- a short string one of the game's
--                                   -- Lua states notes for the others ("" to
--                                   -- forget); note(key) reads it -> string
--                                   -- | nil
--     edgewatch = function(),       -- optional; in a game script's update:
--                                   -- the entities the edge watch reads in
--                                   -- this update, { e, ... } | nil
--     edgewatched = function(e, text), -- optional; in its postUpdate: what
--                                   -- it read of e (tpf3mp/lanes.lua watch)
--     preview = function(action),   -- optional; in the GUI: what the
--                                   -- player's build tool shows now, the
--                                   -- action it would build, or nil once it
--                                   -- shows nothing, for the room's other
--                                   -- members -> true | false, why
--     previews = function(),        -- optional; in the GUI: what the other
--                                   -- members' tools show that changed since
--                                   -- the last call, { { from =, action = } },
--                                   -- no action for one that shows nothing
--     draw    = function(from),     -- optional; in the GUI: the next
--                                   -- makeProposalData draws member from's
--                                   -- preview -> true | false, why
--     drawn   = function(),         -- optional; what it came to -> true |
--                                   -- false, why | nil (nothing evaluated)
--     undraw  = function(from),     -- optional; member from's preview goes
--   }
--
-- An action table mirrors tpf3mp_proto::action::Action field for field, in
-- the game's units: metres, and plain fractions for directions. The hook
-- converts it to and from the schema (tpf3mp_proto::lua), so the rounding,
-- the bounds and the checks live in Rust alone; `command` returns false and
-- a reason for a table the schema refuses.
--
-- `command` is Session::command (docs/HOOKS.md, "The hook's session"). An
-- action the room orders comes back to every game, the one that sent it
-- included, through `take`: the hook hands it to the first simulation update
-- of the step it was ordered for, and the mod's game script
-- (tpf3mp_sim/tpf3mp_sim.script.lua) applies it there, where a command runs
-- at once, so every game applies it in the same update.
--
-- Without the table the game is the plain game, and attach() says so. The
-- mod then does nothing. With a table of another version, or one missing a
-- function, attach() refuses it rather than guessing (fail closed).
--
-- Pure Lua; the tests hand attach() a fake table.

local acceptance = ug_require and ug_require("tpf3mp_1::/scripts/tpf3mp/acceptance.lua")
    or require("tpf3mp.acceptance")

local bridge = {}

-- 13: token-only ordered replay wakes, takeReplay and replayed, so actions
--     run between simulation updates while paused as well as while running.
-- 12: company passwords: `command` takes a password beside the action,
-- which the room seals, and `take` hands each action's seal third;
-- 11: `note`, a short string one of the game's Lua states notes for the
-- others (the two were each 11 on their own branches);
-- 10: companies: `take` also names who sent each action, `status` each
-- player's id (`id`, `me_id`);
-- 9: the Multiplayer window: the room, its chat (`status`, `chat`, `say`);
-- 8: the player hears what became of their actions (`command`'s ticket,
-- `applied`, `results`);
-- 7: the build tools through the room (`clicks`, `replaying`);
-- 6: the game script reads the world's lanes at checkpoints (`checkpoint`,
-- `lanes`);
-- 5: the GUI asks whether the room's game runs (`room`), for the guard;
-- 4: the GUI saves and loads the room's world (`poll`, `saved`, `world`);
-- 3: the room's actions are taken by the game script (`take`); 2 called the
-- GUI's handlers; 1 passed bytes the mod encoded itself.
bridge.VERSION = 15
bridge.GLOBAL = "tpf3mp_native"

local Link = {}
Link.__index = Link

-- The link to the hook, or nil and why there is none.
function bridge.attach(native)
	if native == nil then return nil, "no hook in this game" end
	if type(native) ~= "table" then return nil, bridge.GLOBAL .. " is not a table" end
	if native.version ~= bridge.VERSION then
		return nil, "the hook speaks bridge version " .. tostring(native.version)
			.. ", the mod " .. bridge.VERSION
	end
	for _, name in ipairs({ "command", "take", "log", "poll", "saved", "world", "room",
			"checkpoint", "lanes", "clicks", "replaying", "applied", "results", "status", "chat",
			"say", "takeReplay", "replayed", "scanned" }) do
		if type(native[name]) ~= "function" then
			return nil, "the hook has no " .. name .. "()"
		end
	end
	return setmetatable({ native = native }, Link)
end

-- The hook's table in this state, if the hook gave it one. The hook gives
-- it to a state that has printed, so this prints first. Read through pcall:
-- a state that refuses undeclared globals raises on a missing one.
function bridge.find()
	pcall(print, "[tpf3mp] looking for the hook")
	local ok, value = pcall(function() return tpf3mp_native end)
	if ok then return value end
	return nil
end

-- Hands an action table to the room. Returns true and the action's ticket,
-- which results() names when this game applies the action or never will; or
-- nil and why not: an action that was not handed over must not be applied
-- locally either. `password`, for joining or locking a company only, goes to
-- the room beside it, which orders the action with the password's seal; it
-- is never logged, and no answer quotes it.
function Link:command(action, password)
	if type(action) ~= "table" then return nil, "an action is a table" end
	local allowed, why = acceptance.check(action)
	if not allowed then return nil, why end
	if password ~= nil and type(password) ~= "string" then return nil, "a password is text" end
	local ok, result, reason = pcall(self.native.command, action, password)
	if not ok then return nil, "the hook refused: " .. tostring(result) end
	if result ~= true then
		return nil, "the hook refused the action: " .. tostring(reason or "no reason given")
	end
	return true, reason
end

-- In a game script's postUpdate: what became of the batch's action `index`
-- (from 1), and the entity it made, if any.
function Link:applied(index, ok, entity, why)
	pcall(self.native.applied, index, ok == true, entity, why and tostring(why) or nil)
end

-- A wake carries only a token. Ordered actions come from the hook, once,
-- in the engine's handleEvent, even when no simulation update runs.
function Link:takeReplay(token)
	local ok, actions, origins, seals = pcall(self.native.takeReplay, token)
	if not ok or type(actions) ~= "table" then return nil end
	return actions, origins, seals
end

function Link:replayed(token, ok, why)
	pcall(self.native.replayed, token, ok == true, why and tostring(why) or nil)
end

-- In the GUI: what became of the player's own actions since the last call,
-- a list of { ticket =, ok =, entity =, why = }, oldest first.
function Link:results()
	local ok, results = pcall(self.native.results)
	if not ok or type(results) ~= "table" then return {} end
	return results
end

-- The room, for the Multiplayer window: { room = (display), room_id = (stable), speed =, diverged =,
-- players = { { name =, connected =, owner =, me = } } }, or nil before its
-- game.
function Link:status()
	local ok, status = pcall(self.native.status)
	if not ok or type(status) ~= "table" then return nil end
	return status
end

-- What the room's members said since the last call, oldest first:
-- { { from =, text = } }.
function Link:chat()
	local ok, heard = pcall(self.native.chat)
	if not ok or type(heard) ~= "table" then return {} end
	return heard
end

-- Says `text` to the room for the player: true, or nil and why not.
function Link:say(text)
	local ok, said, why = pcall(self.native.say, tostring(text))
	if not ok then return nil, tostring(said) end
	if said ~= true then return nil, tostring(why or "the hook did not take it") end
	return true
end

-- Puts `text`, the room's invite code, on the clipboard: true, or nil and
-- why not.
function Link:copy(text)
	if type(self.native.copy) ~= "function" then return nil, "this hook cannot copy" end
	local ok, copied, why = pcall(self.native.copy, tostring(text))
	if not ok then return nil, tostring(copied) end
	if copied ~= true then return nil, tostring(why or "the hook did not copy it") end
	return true
end

-- The actions the room ordered for this update, as a list, or nil; who
-- sent each, a list of player ids (64 hex digits) beside it; and the seal of
-- the password each was sent with, { scope =, tag = }, or false.
function Link:take()
	local ok, actions, origins, seals = pcall(self.native.take)
	if not ok or type(actions) ~= "table" then return nil end
	if type(origins) ~= "table" then origins = {} end
	if type(seals) ~= "table" then seals = {} end
	return actions, origins, seals
end

function Link:log(line)
	pcall(self.native.log, tostring(line))
end

-- Notes `value` (a string; "" forgets it) under `key` for the game's other
-- Lua states, or, without a value, reads what one noted: a string or nil.
function Link:note(key, value)
	if value ~= nil then
		pcall(self.native.note, tostring(key), tostring(value))
		return nil
	end
	local ok, noted = pcall(self.native.note, tostring(key))
	if ok and type(noted) == "string" then return noted end
	return nil
end

-- What the hook asks of the game, once: { save = name }, { load = name },
-- or nil.
function Link:poll()
	local ok, request = pcall(self.native.poll)
	if not ok or type(request) ~= "table" then return nil end
	return request
end

-- Answers a save the hook asked for.
function Link:saved(name, ok, why)
	pcall(self.native.saved, tostring(name), ok == true, why and tostring(why) or nil)
end

-- A world's GUI started: after a load the hook asked for, the world loaded.
function Link:world()
	pcall(self.native.world)
end

-- The seed for math.randomseed in this update: the room step's, or nil
-- outside the room's steps (or from a hook without it).
function Link:seed()
	if type(self.native.seed) ~= "function" then return nil end
	local ok, seed = pcall(self.native.seed)
	if ok and type(seed) == "number" then return seed end
	return nil
end

-- Whether this update reports a checkpoint, and the absolute simulation step
-- whose rolling sample must be read. No step is returned outside room updates.
function Link:checkpoint()
	local ok, due, step = pcall(self.native.checkpoint)
	return ok and due == true, ok and step or nil
end

function Link:scanned(ok, why, ms)
	return self.native.scanned(ok, why, ms)
end

-- Hands the lanes read at a checkpoint to the hook. Returns true, or nil
-- and why not; lanes not handed over hold the world.
function Link:lanes(lanes)
	local ok, taken, why = pcall(self.native.lanes, lanes)
	if not ok then return nil, "the hook refused: " .. tostring(taken) end
	if taken ~= true then return nil, tostring(why or "the hook refused the lanes") end
	return true
end

-- In the GUI: click `click` (the count before it) was handed to the room
-- with `ticket`, or not (nil): the street and track tools continue from
-- what the room built for it (tpf3mp/toolchain.lua). A hook without it
-- leaves the tools as they are.
function Link:chainClick(click, ticket)
	if type(self.native.chainClick) ~= "function" then return end
	pcall(self.native.chainClick, click, ticket)
end

-- In the GUI, each frame: the edge the room built where the player's last
-- street or track click ended, to check in the GUI's state (edge, node0,
-- node1, end, x, y, z), or nothing.
function Link:chainPoll()
	if type(self.native.chainPoll) ~= "function" then return nil end
	local ok, edge, node0, node1, endIndex, x, y, z = pcall(self.native.chainPoll)
	if not ok or type(edge) ~= "number" then return nil end
	return edge, node0, node1, endIndex, x, y, z
end

-- In the GUI: its state has `edge` as the room built it, with `streets`
-- street edges and `others` other edges at the clicked end's node.
function Link:chainSeen(edge, streets, others)
	if type(self.native.chainSeen) ~= "function" then return end
	pcall(self.native.chainSeen, edge, streets, others)
end

-- The player's builds queued in the room's game so far, or nil where the
-- hook cannot take them to the room (the tools then stay refused).
function Link:clicks()
	local ok, clicks = pcall(self.native.clicks)
	if ok and type(clicks) == "number" then return clicks end
	return nil
end

-- In the GUI: the build the module editor queued at click `click` (the
-- count before it), read by the hook, as game scripts see a proposal; nil
-- and why when it did not read; nil when that click was not the module
-- editor's, or the hook has no `built` (it is optional: the module editor
-- then stays refused).
function Link:built(click)
	if type(self.native.built) ~= "function" then return nil end
	local ok, proposal, why = pcall(self.native.built, click)
	if not ok then return nil, "the hook refused: " .. tostring(proposal) end
	if type(proposal) == "table" then return proposal end
	if why ~= nil then return nil, tostring(why) end
	return nil
end

-- In the GUI: the level crossing type the street or track tool's click
-- `click` lays its crossings as, read by the hook from the click's
-- proposal (docs/HOOKS.md, "Level crossing types"), once: the type (this
-- game's index); false when it picked none; false and why when the hook
-- cannot tell (no `toolCrossing`, which is optional, or a profile without
-- the maps: the GUI then refuses the build); nil and why when it did not
-- read; nil when nothing was kept for the click.
function Link:toolCrossing(click)
	if type(self.native.toolCrossing) ~= "function" then
		return false, "the hook does not read the tools' level crossing type"
	end
	local ok, kind, why = pcall(self.native.toolCrossing, click)
	if not ok then return nil, "the hook refused: " .. tostring(kind) end
	if type(kind) == "number" then return kind end
	if kind == false then return false, why ~= nil and tostring(why) or nil end
	if why ~= nil then return nil, tostring(why) end
	return nil
end

-- In a game script's postUpdate at a checkpoint: the lanes the hook wants
-- dumped entry by entry (docs/HOOKS.md, "Lane dumps"), { step =, lanes = {
-- n, ... } }, once; or nil, and nil from a hook without dumps (`dump` is
-- optional).
function Link:dump()
	if type(self.native.dump) ~= "function" then return nil end
	local ok, order = pcall(self.native.dump)
	if not ok or type(order) ~= "table" or type(order.lanes) ~= "table" then return nil end
	return order
end

-- Hands the hook one entry of a lane dumped. Returns whether it was taken:
-- false once the checkpoint has written its most.
function Link:dumped(lane, entry)
	if type(self.native.dumped) ~= "function" then return false end
	local ok, taken = pcall(self.native.dumped, lane, tostring(entry))
	return ok and taken == true
end

-- In a game script's update: the entities the edge watch reads in this
-- update (docs/HOOKS.md, "The edge watch"), a list; or nil, and nil from a
-- hook without the watch (`edgewatch` is optional).
function Link:edgewatch()
	if type(self.native.edgewatch) ~= "function" then return nil end
	local ok, list = pcall(self.native.edgewatch)
	if ok and type(list) == "table" and #list > 0 then return list end
	return nil
end

-- In the GUI: the player's build tool shows `action` now (an action table,
-- as command takes one), or nothing (nil), for the room's other members to
-- see (docs/HOOKS.md, "Build previews"). Never applied anywhere. Returns
-- true, or nil and why; nil from a hook without `preview` (it is optional:
-- the other members then see nothing).
function Link:preview(action)
	if type(self.native.preview) ~= "function" then return nil, "this hook shows no previews" end
	if action ~= nil and type(action) ~= "table" then return nil, "a preview is an action table" end
	local ok, shown, why = pcall(self.native.preview, action)
	if not ok then return nil, "the hook refused: " .. tostring(shown) end
	if shown ~= true then return nil, tostring(why or "the hook did not take it") end
	return true
end

-- In the GUI: what the other members' build tools show that changed since
-- the last call, { { from =, action = } }, `from` a player id (64 hex
-- digits), no `action` for one that shows nothing now; {} from a hook
-- without `previews`.
function Link:previews()
	if type(self.native.previews) ~= "function" then return {} end
	local ok, changes = pcall(self.native.previews)
	if not ok or type(changes) ~= "table" then return {} end
	return changes
end

-- In the GUI: draws member `from`'s preview, the proposal `proposal` with
-- `context`, in the hook's renderer for them (docs/HOOKS.md, "Build
-- previews"): the hook draws what the game evaluates for it with `evaluate`
-- (api.engine.util.proposal.makeProposalData). True and what `evaluate`
-- answered (the ProposalData), or nil and why; nil from a hook that cannot
-- draw (`draw` is optional).
function Link:drawPreview(from, proposal, context, evaluate)
	local native = self.native
	if type(native.draw) ~= "function" or type(native.drawn) ~= "function" then
		return nil, "this hook draws no previews"
	end
	local ok, armed, why = pcall(native.draw, tostring(from))
	if not ok then return nil, tostring(armed) end
	if armed ~= true then return nil, tostring(why or "the hook did not arm") end
	local evaluated, err = pcall(evaluate, proposal, context)
	local okDrawn, drawn, whyNot = pcall(native.drawn)
	if not evaluated then return nil, "the game did not evaluate it: " .. tostring(err) end
	if not okDrawn then return nil, tostring(drawn) end
	if drawn == nil then return nil, "the game made nothing to draw" end
	if drawn ~= true then return nil, tostring(whyNot or "not drawn") end
	return true, err
end

-- In the GUI: member `from`'s preview goes.
function Link:undrawPreview(from)
	if type(self.native.undraw) ~= "function" then return end
	pcall(self.native.undraw, tostring(from))
end

-- Hands the hook what the edge watch read of `entity`.
function Link:edgewatched(entity, text)
	if type(self.native.edgewatched) ~= "function" then return end
	pcall(self.native.edgewatched, entity, tostring(text))
end

-- Names in a text, one a line.
local function lines(text)
	local out = {}
	if type(text) ~= "string" then return out end
	for name in string.gmatch(text, "[^\n]+") do out[#out + 1] = name end
	return out
end

-- Whether the room gave the mods its worlds load with (docs/MODS.md); false
-- from a hook without `mods` (it is optional).
function Link:hasMods()
	if type(self.native.mods) ~= "function" then return false end
	local ok, known = pcall(self.native.mods)
	return ok and known == true
end

-- The mods to load a save whose mods are `names` with, as a list of names,
-- then those left out and those added; nil when the room gave no lists (the
-- save loads with its own).
function Link:mods(names)
	if type(self.native.mods) ~= "function" then return nil end
	local ok, plan, dropped, added = pcall(self.native.mods, table.concat(names, "\n"))
	if not ok or type(plan) ~= "string" then return nil end
	return lines(plan), lines(dropped), lines(added)
end

-- The settings of the room's mods its owner picked, by mod, by setting, the
-- game's own under ""; nil from a hook without `modparams` or with none (the
-- save's then stay).
function Link:modParams()
	if type(self.native.modparams) ~= "function" then return nil end
	local ok, text = pcall(self.native.modparams)
	if not ok or type(text) ~= "string" or text == "" then return nil end
	local room = {}
	for mod, key, value in string.gmatch(text, "([^\t\n]*)\t([^\t\n]+)\t(%-?%d+)") do
		room[mod] = room[mod] or {}
		room[mod][key] = math.floor(tonumber(value))
	end
	return room
end

-- This player's personal mods, by name, as a set; an empty set from a hook
-- without `personal` or without the room's lists.
function Link:personal()
	local set = {}
	if type(self.native.personal) ~= "function" then return set end
	local ok, text = pcall(self.native.personal)
	if not ok then return set end
	for _, name in ipairs(lines(text)) do set[name] = true end
	return set
end

-- The room's shared mods, as a list of names; nil from a hook without
-- `shared` or without the room's lists.
function Link:shared()
	if type(self.native.shared) ~= "function" then return nil end
	local ok, text = pcall(self.native.shared)
	if not ok or type(text) ~= "string" then return nil end
	return lines(text)
end

-- The game script begins (true) or ends applying the room's actions.
function Link:replaying(on)
	pcall(self.native.replaying, on == true)
end

-- While the room's actions run: arms the next build sent with the terraform
-- grid `grid` (true, or nil and why), or with nil disarms, answering whether
-- a build was filled (nil when none was armed). A hook without `terrain` (it
-- is optional) applies no terraform.
function Link:terrain(grid)
	if type(self.native.terrain) ~= "function" then return nil, "this hook cannot apply a terraform" end
	local ok, result, why = pcall(self.native.terrain, grid)
	if not ok then return nil, "the hook refused: " .. tostring(result) end
	return result, why
end

-- While the room's actions run: arms the next build sent with level crossing
-- `entity` taking type `index` (true, or nil and why), or with no arguments
-- disarms, answering whether a build was filled (nil when none was armed).
-- A hook without `crossing` (it is optional) sets no crossing's type.
function Link:crossing(entity, index)
	if type(self.native.crossing) ~= "function" then return nil, "this hook cannot set a crossing's type" end
	local ok, result, why
	if entity == nil then
		ok, result, why = pcall(self.native.crossing)
	else
		ok, result, why = pcall(self.native.crossing, entity, index)
	end
	if not ok then return nil, "the hook refused: " .. tostring(result) end
	return result, why
end

-- Whether the room's game runs. A hook that cannot say is taken to say yes:
-- the guard then refuses rather than lets a command through unchecked.
function Link:room()
	local ok, inRoom = pcall(self.native.room)
	if not ok then return true end
	return inRoom == true
end

return bridge
