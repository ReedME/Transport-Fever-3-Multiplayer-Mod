-- tpf3mp/previews.lua -- what the players' build tools show, in a room's
-- game (docs/HOOKS.md, "Build previews"): the player's own goes to the
-- room's other members, and theirs come here to be shown. Advisory: nothing
-- here builds, sends a command or touches the world.
--
-- - **Out.** The game script's GUI half hands over the action each of the
--   player's tool proposals would build, as the capture made it
--   (tpf3mp/capture.lua), for the tools whose builds can be shown (SHOWN):
--   `shown`. It hides it when the tool shows nothing (an empty proposal, a
--   proposal the room cannot carry, a click, which the room then orders as
--   the real build) and when the game's own list of active tools is no
--   longer what it was at the tool's last proposal (`tick`): build 40408
--   names its tools there by their window ("Construction",
--   "variant-tracks"), never by the event's id ("streetBuilder"), so a
--   change of the list is the tool closing or another opening. The hook
--   sends it on, at most five times a second, and again every two seconds
--   while it shows (crates/tpf3mp-hook/src/previews.rs).
-- - **In.** The Multiplayer plugin (gui/tpf3mp/tpf3mp.script.lua), which
--   stays mounted in the game bar, takes what changed of the other members'
--   previews (`take`) and makes each into the proposal it would build in
--   this game (tpf3mp/apply.lua, apply.proposalOf), never sent, and has
--   the hook draw it (`draw`, given by the plugin: the hook's renderer for
--   that member, crates/tpf3mp-hook/src/drawing.rs), in 3D as the tools
--   draw theirs; one gone it clears. The game's own
--   `builtin.ProposalViewer` cannot: build 40408 allows it only inside a
--   tool's ActionDescriptor and fails fatally anywhere else
--   (`!IsTransformWithContext`). Each member's first preview, the first
--   drawn, and why one does not show are said in the log, and what this
--   game says of a member's preview each time that changes (`verdict`): a
--   preview drawn red is one this game calls critical or finds errors in.
--
-- Ported from TpF2 Multiplayer's shared build previews (mp/previews.lua in
-- tpf2-multiplayer), on the room's own action schema.
--
-- Pure Lua; the tests hand it a fake link and a fake api.

local previews = {}

-- The tools whose previews the other members are shown, by the capture's
-- kind: new constructions (stations, depots, buildings, a station's edit),
-- streets, tracks and stops. Not the bulldozer's removals, nor the
-- modifiers' and junction tools' changes, which show nothing new.
previews.SHOWN = { construction = true, street = true, track = true, stop = true }

-- Seconds between two looks at the game's active tools.
local TOOL_EVERY = 0.25
-- Seconds between two tries at drawing the previews the hook could not
-- draw yet (all its renderers busy): a member's preview that does not
-- change comes again as no change, so it is tried again from here.
local RETRY_EVERY = 0.5
-- Members whose first preview the log says, at most.
local MAX_SAID = 16

-- What the player's tool shows: the tool's id and the game's list of
-- active tools at its last proposal (`tools`, nil where the game cannot
-- say: then only its next proposal or click hides it).
local showing = nil
-- When the active tools were last looked at.
local lookedAt = nil
-- The other members' previews, by player id: { proposal =, context =,
-- company =, kind =, seq = }; and whose first was said, and drawn.
local remote, said, saidCount, drawnSaid = {}, {}, 0, {}
-- Counts the previews made, so each has a viewer id of its own.
local made = 0
-- Previews this game could not make, said in the log, at most.
local MAX_UNMADE = 20
local unmade = 0
-- What this game said of each member's preview last (`verdict`), by player
-- id, and how many such lines the log has had, at most MAX_VERDICTS.
local MAX_VERDICTS = 40
local verdicts, verdictsSaid = {}, 0
-- What the log said of the active tools' list, once.
local toldTools = false
-- When the previews not drawn yet were last tried again.
local retriedAt = nil
-- What making and drawing the other members' previews cost here, for the
-- log once every COST_EVERY seconds while any came (os.clock: elapsed time
-- on Windows, CPU time on Linux and macOS):
-- the previews made, drawn and retried, each { n =, ms =, max = }, and the
-- parts of the longest make (tpf3mp/apply.lua, apply.lastCost).
local COST_EVERY = 10
local function newCost() return { make = { n = 0, ms = 0, max = 0 }, draw = { n = 0, ms = 0, max = 0 },
	retry = { n = 0, ms = 0, max = 0 } } end
local cost, costSince = newCost(), nil

-- The game's active tools, as one string, the same whatever their order,
-- or nil where it cannot say.
local function activeTools(api)
	local ok, ids = pcall(function() return api.gui.contextHelper.getIdsOfActiveTool() end)
	if not ok or type(ids) ~= "table" then return nil end
	local names = {}
	for i, id in ipairs(ids) do names[i] = tostring(id) end
	table.sort(names)
	return table.concat(names, ", ")
end

local function now()
	local ok, t = pcall(os.clock)
	return ok and t or 0
end

local function spent(what, t0)
	local ms = (now() - t0) * 1000
	if ms < 0 then return 0 end
	local c = cost[what]
	c.n, c.ms = c.n + 1, c.ms + ms
	if ms > c.max then c.max = ms end
	return ms
end

local function costLine(link, t)
	if costSince == nil or t < costSince then costSince = t end
	if t - costSince < COST_EVERY then return end
	local c = cost
	if c.make.n + c.draw.n + c.retry.n > 0 then
		local function part(name, p) return string.format("%s %d in %.1f ms (longest %.1f)", name, p.n, p.ms, p.max) end
		link:log(string.format("build previews in the last %.1f s: ", t - costSince)
			.. part("made", c.make) .. (c.slowest and (" [" .. c.slowest .. "]") or "") .. ", "
			.. part("drawn", c.draw) .. ", " .. part("retried", c.retry))
	end
	cost, costSince = newCost(), t
end

-- The player's tool `tool` (its id) proposes `action`, as the capture made
-- it for the tool's `kind`: shown to the other members when the kind is
-- one SHOWN, else whatever showed is hidden.
function previews.shown(link, api, tool, kind, action)
	if not previews.SHOWN[kind] or type(action) ~= "table" then
		previews.hidden(link)
		return
	end
	local ok, why = link:preview(action)
	if not ok then
		-- Too large, or a hook without previews: the others see nothing.
		if showing ~= nil then previews.hidden(link) end
		return why
	end
	-- The list as the tool shows this proposal: one that changes later is
	-- the tool closing, or another opening.
	local tools = activeTools(api)
	if showing == nil or showing.tool ~= tool then
		if not toldTools then
			toldTools = true
			link:log("the game's active tools as the " .. tostring(tool) .. " shows a preview: "
				.. (tools ~= nil and "(" .. tools .. "); it hides once they change"
					or "the game does not say; it hides on its next proposal or click only"))
		end
	end
	showing = { tool = tool, tools = tools }
	return nil
end

-- The player's tool shows nothing now.
function previews.hidden(link)
	if showing == nil then return end
	showing = nil
	link:preview(nil)
end

-- Once a GUI frame in the room's game, in the game script's GUI half:
-- hides the player's preview once the game's active tools are no longer
-- those of its last proposal.
function previews.tick(link, api)
	if showing == nil or showing.tools == nil then return end
	local t = now()
	if lookedAt == nil or t - lookedAt >= TOOL_EVERY or t < lookedAt then
		lookedAt = t
		local tools = activeTools(api)
		if tools ~= showing.tools then previews.hidden(link) end
	end
end

-- What an entity a preview collides with is, where the game says: its
-- kind of component, or nil.
local KINDS = { { "BASE_EDGE", "edge" }, { "BASE_NODE", "node" }, { "CONSTRUCTION", "construction" },
	{ "TOWN_BUILDING", "town building" } }
local function kindOf(entity)
	if type(api) ~= "table" then return nil end
	for _, k in ipairs(KINDS) do
		local ok, found = pcall(function()
			return api.engine.getComponent(entity, api.type.ComponentType[k[1]]) ~= nil
		end)
		if ok and found then return k[2] end
	end
	return nil
end

-- What the game says of a proposal, from the ProposalData it made for it:
-- "critical" where it would refuse it outright, its error messages and
-- warnings, and the entities it collides with, by kind; "fine" for none;
-- nil where there is no ProposalData to read.
function previews.verdict(data)
	if data == nil then return nil end
	local function list(t)
		local out = {}
		local ok = pcall(function()
			for _, v in ipairs(t or {}) do out[#out + 1] = tostring(v) end
		end)
		return ok and out or {}
	end
	local parts = {}
	local critical, messages, warnings, colliding = false, {}, {}, {}
	pcall(function()
		local state = data.errorState
		if state then
			critical = state.critical == true
			messages = list(state.messages)
			warnings = list(state.warnings)
		end
	end)
	pcall(function()
		for _, e in ipairs(data.collisionInfo and data.collisionInfo.collisionEntities or {}) do
			-- An EntityData: userdata in the game, a table in the tests.
			local ok, entity = pcall(function() return e.entity end)
			if not ok or entity == nil then entity = e end
			colliding[#colliding + 1] = entity
		end
	end)
	if critical then parts[#parts + 1] = "critical" end
	if #messages > 0 then parts[#parts + 1] = "errors " .. table.concat(messages, "; ") end
	if #warnings > 0 then parts[#parts + 1] = "warnings " .. table.concat(warnings, "; ") end
	if #colliding > 0 then
		-- Each kind's count and its first few entities.
		local kinds, order = {}, {}
		for _, entity in ipairs(colliding) do
			local kind = kindOf(entity) or "entity"
			if kinds[kind] == nil then
				kinds[kind] = { n = 0, ids = {} }
				order[#order + 1] = kind
			end
			local k = kinds[kind]
			k.n = k.n + 1
			if #k.ids < 4 then k.ids[#k.ids + 1] = tostring(entity) end
		end
		local said = {}
		for _, kind in ipairs(order) do
			local k = kinds[kind]
			said[#said + 1] = k.n .. " " .. kind .. " (" .. table.concat(k.ids, ",")
				.. (k.n > #k.ids and ",..." or "") .. ")"
		end
		parts[#parts + 1] = "collides with " .. table.concat(said, ", ")
	end
	if #parts == 0 then return "fine" end
	return table.concat(parts, ", ")
end

-- What this game says of member `from`'s `kind` preview, in the log when
-- it changed, a few dozen times.
local function judged(link, from, kind, data)
	local verdict = previews.verdict(data)
	if verdict == nil or verdicts[from] == verdict then return end
	verdicts[from] = verdict
	if verdictsSaid >= MAX_VERDICTS then return end
	verdictsSaid = verdictsSaid + 1
	link:log("another member's " .. tostring(kind) .. " preview, as this game sees it: " .. verdict)
end

-- A preview that does not show here, said in the log a few dozen times.
local function unshown(link, kind, why)
	if unmade >= MAX_UNMADE then return end
	unmade = unmade + 1
	link:log("another member's " .. tostring(kind) .. " preview does not show here: " .. tostring(why))
end

-- In the Multiplayer plugin, once a frame: takes what changed of the other
-- members' previews and makes each into its proposal with
-- `make(action, from)`, which answers the proposal, its context and the
-- company it is built for, or nil and why; then has `draw(from, kept)`
-- draw it, or with nil clear it, which answers true, or nil and why.
-- Returns whether any changed.
function previews.take(link, make, draw)
	local changed = false
	for _, change in ipairs(link:previews()) do
		local from, action = change.from, change.action
		if type(from) == "string" then
			changed = true
			local had = remote[from] ~= nil
			remote[from] = nil
			if type(action) ~= "table" and had and draw then pcall(draw, from, nil) end
			if type(action) == "table" then
				local kind = next(action)
				if not said[from] and saidCount < MAX_SAID then
					said[from], saidCount = true, saidCount + 1
					link:log("another member's build preview arrived: " .. tostring(kind) .. " from " .. from:sub(1, 16))
				end
				local t0 = now()
				local ok, proposal, context, company = pcall(make, action, from)
				local ms = spent("make", t0)
				if ms >= cost.make.max then
					local okA, applyModule = pcall(require, "tpf3mp.apply")
					local parts = okA and type(applyModule) == "table" and applyModule.costText
						and applyModule.costText(applyModule.lastCost) or ""
					cost.slowest = tostring(kind) .. (parts ~= "" and (": " .. parts) or "")
				end
				if ok and proposal ~= nil then
					made = made + 1
					local kept = { proposal = proposal, context = context, company = company,
						kind = kind, seq = made }
					remote[from] = kept
					if draw then
						local d0 = now()
						local called, drawn, why = pcall(draw, from, kept)
						spent("draw", d0)
						if called and drawn then
							kept.undrawn = nil
							judged(link, from, kind, why)
							if not drawnSaid[from] then
								drawnSaid[from] = true
								link:log("drawing another member's build preview: " .. tostring(kind)
									.. " from " .. from:sub(1, 16))
							end
						else
							if had then pcall(draw, from, nil) end
							kept.undrawn = true
							unshown(link, kind, called and why or drawn)
						end
					end
				else
					if had and draw then pcall(draw, from, nil) end
					unshown(link, kind, ok and context or proposal)
				end
			end
		end
	end
	-- The ones made but not drawn, tried again now and then: a renderer may
	-- have come free since.
	if draw then
		local t = now()
		if retriedAt == nil or t - retriedAt >= RETRY_EVERY or t < retriedAt then
			retriedAt = t
			for from, kept in pairs(remote) do
				if kept.undrawn then
					local r0 = now()
					local called, drawn, data = pcall(draw, from, kept)
					spent("retry", r0)
					if called and drawn then
						kept.undrawn = nil
						judged(link, from, kept.kind, data)
					end
				end
			end
		end
	end
	costLine(link, now())
	return changed
end

-- The other members' previews as kept, by player id (for the tests).
function previews.remote()
	return remote
end

-- Forgets everything: a new world's GUI.
function previews.reset()
	showing, lookedAt, remote, said, saidCount, toldTools = nil, nil, {}, {}, 0, false
	retriedAt = nil
	made, unmade, drawnSaid = 0, 0, {}
	verdicts, verdictsSaid = {}, 0
	cost, costSince = newCost(), nil
end

return previews
