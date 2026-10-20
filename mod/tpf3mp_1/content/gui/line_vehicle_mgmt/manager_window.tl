-- TPF3-MP: Transport Fever 3's gui/line_vehicle_mgmt/manager_window.tl (build
-- 40420), served in its place by the hook's menu patch
-- (crates/tpf3mp-hook/src/menu_entry.rs), with one change, marked TPF3-MP:
-- stations clicked while a new line is being created are added to it once it
-- exists, instead of being dropped. Refresh this copy from the game's when a
-- supported build changes the game's file.
local app_util = ug_require "/scripts/app_util.tl" as AppUtil
local builtin = ug_require "::/gui/main/builtin.lua" as Builtin
local button_react_util = ug_require "/gui/main/button_react_util.tl" as ButtonReactUtil
local cargo_util = ug_require "/gui/main/cargo_util.tl" as CargoUtil
local CargoFilterWindow = ug_require "/gui/line_vehicle_mgmt/cargofilter_window.tl" as Recipe<CargoFilterUtil.CargoFilterParams>
local color_util = ug_require "/gui/main/color_util.tl" as ColorUtil
local engine_react_util = ug_require "/gui/main/engine_react_util.tl" as EngineReactUtil
local entity_util = ug_require "/scripts/entity_util.tl" as EntityUtil
local feedback_list_util = ug_require "::/gui/line_vehicle_mgmt/feedback_list_util.tl" as FeedbackListUtil
local gui_react_util = ug_require "/gui/main/gui_react_util.tl" as GuiReactUtil
local lang_util = ug_require "/scripts/lang_util.tl" as LangUtil
local line_react_util = ug_require "line_react_util.tl" as LineReactUtil
local line_util = ug_require "/gui/line_vehicle_mgmt/line_util.tl" as LineUtil
local LineManagerPanel = ug_require "/gui/line_vehicle_mgmt/line_manager_panel.tl" as Recipe<LineManagerPanelParams>
local manager_hud_util = ug_require "/gui/line_vehicle_mgmt/manager_hud_util.tl" as ManagerHudUtil
local manager_tooltips_util = ug_require "/gui/line_vehicle_mgmt/manager_tooltips_util.tl" as ManagerTooltipsUtil
local math_util = ug_require "::/scripts/mathutil.lua" as MathUtil
local popover_react_util = ug_require "/gui/main/popover_react_util.tl" as PopoverReactUtil
local react = ug_require "::/gui/main/react.lua" as React
local statistics_react_util = ug_require "/gui/statistics/statistics_react_util.tl" as StatisticsReactUtil
local styleutil = ug_require "/gui/main/styleutil.tl" as StyleUtil
local table_util = ug_require "/scripts/table_util.tl" as TableUtil
local util = ug_require "/scripts/util.tl" as Util
local vehicle_list_react_util = ug_require "vehicle_list_react_util.tl" as VehicleListReactUtil
local vehicle_react_util = ug_require "vehicle_react_util.tl" as VehicleReactUtil
local vehicle_store_window = ug_require "/gui/line_vehicle_mgmt/vehicle_store_window.tl" as VehicleStoreWindow
local vehicle_store_util = ug_require "/gui/line_vehicle_mgmt/vehicle_store_util.tl" as VehicleStoreUtil
local vehicle_util = ug_require "/gui/line_vehicle_mgmt/vehicle_util.tl" as VehicleUtil

local lvmSFX = api.gui.genericRep.get(api.gui.genericRep.find("::/gui/line_vehicle_mgmt/sound/lvm_sfx.gres")).data as LvmSFX
local guiCompSFX = api.gui.genericRep.get(api.gui.genericRep.find("::/gui/sound/gui_comp_sfx.gres")).data as GuiCompSFX

local function isEnabledReturnTrue() : boolean
	return true
end

local function isEnabledReturnFalse() : boolean
	return false
end

local function handlerNoOp()
end

local function pickFunctionForConstructionStation(
	conEntity : Engine.Entity,
	construction : Engine.Component.Construction,
	pickedStation : function(sDetails : LineUtil.StationSelectionDetails)
)
	if construction and construction.stations and entity_util.isOwnedByPlayerOrNotOwned(conEntity) then
		for __, conStation in ipairs(construction.stations) do
			local conStationGroup = api.engine.system.stationGroupSystem.getStationGroup(conStation)
			if conStationGroup then
				pickedStation({
					stationGroup = conStationGroup,
				})
				break
			end
		end
	end
end

local function getSelectedVehicleEntities(vehicleManagerStateRef : ReactRefT<VehicleManagerState>) : {Engine.Entity}
	local selectedVehicles : {Engine.Entity} = {}
	local vEARs = vehicleManagerStateRef:get().vehicleListEntitiesSelected
	for __, vEAR in ipairs(vEARs) do
		table.insert(selectedVehicles, vEAR.entity)
	end
	return selectedVehicles
end

local makeFilteredLinesForSendToLine = function(
	selectedVehicles : {Engine.Entity},
	onlyVisible : boolean
) : {Engine.Entity}
	local commonCarriers : {Carrier} = {}
	local hasLightRailTram = false
	local hasLightRailTrain = false
	local onlyHelicopters = false
	local onlyPlanes = false

	local tmHelicopter = api.type["enum"].TransportMode.HELICOPTER
	local tmAircraft   = api.type["enum"].TransportMode.AIRCRAFT
	local tmSmAircraft = api.type["enum"].TransportMode.SMALL_AIRCRAFT
	local tmTram       = api.type["enum"].TransportMode.TRAM
	local tmTramTr     = api.type["enum"].TransportMode.TRAM_TRACK
	local tmETram      = api.type["enum"].TransportMode.ELECTRIC_TRAM
	local tmETramTr    = api.type["enum"].TransportMode.ELECTRIC_TRAM_TRACK

	for __, selectedVehicle in ipairs(selectedVehicles) do
		if api.engine.entityExists(selectedVehicle) then
			local tv = api.engine.getComponent(selectedVehicle, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle

			if tv then
				if not table_util.arrayContains(commonCarriers, tv.carrier) then
					table.insert(commonCarriers, tv.carrier)
				end

				local transportModeMapVehicle = vehicle_store_util.getTransportModeMapForVehicles({selectedVehicle})
				if not hasLightRailTram and tv.carrier == api.type["enum"].Carrier.TRAM then
					if transportModeMapVehicle[tmTramTr] then
						hasLightRailTram = true
					elseif transportModeMapVehicle[tmETramTr] then
						hasLightRailTram = true
					end
				end
				if not hasLightRailTrain and tv.carrier == api.type["enum"].Carrier.RAIL then
					if transportModeMapVehicle[tmTramTr] then
						hasLightRailTrain = true
					elseif transportModeMapVehicle[tmETramTr] then
						hasLightRailTrain = true
					elseif transportModeMapVehicle[tmTram] then
						hasLightRailTrain = true
					elseif transportModeMapVehicle[tmETram] then
						hasLightRailTrain = true
					end
				end
			end
		end
	end

	local transportModeMapAll = vehicle_store_util.getTransportModeMapForVehicles(selectedVehicles)

	for __, commonCarrier in ipairs(commonCarriers) do
		if commonCarrier == api.type["enum"].Carrier.AIR then
			onlyPlanes = true
			onlyHelicopters = true

			if not transportModeMapAll[tmHelicopter] then
				onlyHelicopters = false
			end

			if not (transportModeMapAll[tmAircraft] or transportModeMapAll[tmSmAircraft]) then
				onlyPlanes = false
			end

			if transportModeMapAll[tmHelicopter] and (transportModeMapAll[tmAircraft] or transportModeMapAll[tmSmAircraft]) then

				onlyHelicopters = false
				onlyPlanes = false
			end
		end
	end

	if hasLightRailTrain then
		if not table_util.arrayContains(commonCarriers, api.type["enum"].Carrier.TRAM) then
			table.insert(commonCarriers, api.type["enum"].Carrier.TRAM)
		end
	end

	local allPlayerLines = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())

	local filteredLines : {Engine.Entity} = {}
	for __, lineEntity in ipairs(allPlayerLines) do
		if api.engine.entityExists(lineEntity) then
			if not onlyVisible or api.gui.byEntity.isLineEmptyOrVisible(lineEntity) then
				if api.engine.util.line.isLineCompatibleWithAnyCarrier(lineEntity, commonCarriers, false) then
					local showLine = true
					if table_util.arrayContains(commonCarriers, api.type["enum"].Carrier.TRAM) and not (hasLightRailTram or hasLightRailTrain) then
						if api.engine.util.line.isLineCompatibleWithCarrier(lineEntity, api.type["enum"].Carrier.RAIL, false) then
							showLine = false
						end
					end
					if table_util.arrayContains(commonCarriers, api.type["enum"].Carrier.AIR) then
						if onlyPlanes then
							showLine = (line_util.isLineCompatibleWithTransportMode(lineEntity, tmSmAircraft)
								or line_util.isLineCompatibleWithTransportMode(lineEntity, tmAircraft))
						elseif onlyHelicopters then
							showLine = line_util.isLineCompatibleWithTransportMode(lineEntity, tmHelicopter)
						end
					end
					if showLine then
						table.insert(filteredLines, lineEntity)
					end
				end
			end
		end
	end

	return filteredLines
end

local function actionHandleVehicles(
	ctx : LineUtil.ContextBuilder,
	commonParams : CommonActionParams,
	entity : Engine.Entity
)
	if not entity_util.isOwnedByPlayer(entity) then
		return
	end

	local isVehicleInVehicleList = commonParams.isVehicleInVehicleList(entity)
	local isVehicleSelected = commonParams.isVehicleSelected(entity)
	if isVehicleInVehicleList then
		if isVehicleSelected then
			ctx.tooltip(manager_tooltips_util.RemoveVehicleFromSelection, entity)
		else
			ctx.tooltip(manager_tooltips_util.AddVehicleToSelection, entity)
		end
	else
		ctx.tooltip(manager_tooltips_util.AddVehicleToList, entity)
	end

	ctx.iaClickOrSelect(
		function()
			if isVehicleInVehicleList then
				commonParams.selectOrDeselectVehicleInVehicleList(
					entity,
					not isVehicleSelected
				)
				api.gui.sound.playRandomSoundEffect(guiCompSFX.CheckboxCheck) -- yes, we want this, as it should sound as in the list in 2d
			else
				commonParams.addOrRemoveVehicleToVehicleList({
					entity = entity,
					removeFromList = false,
				})
				api.gui.sound.playRandomSoundEffect(guiCompSFX.CheckboxUncheck) -- yes, we want this, as it should sound as in the list in 2d
			end
		end,
		{
			"compareAddOrRemoveVehicleToVehicleList",
			entity,
			isVehicleInVehicleList,
			isVehicleSelected,
		},
		isEnabledReturnTrue,
		"compareIsEnabledTrue"
	)
end

local function makeActionMainMode(commonParams : CommonActionParams, __any : any) : TreeNodeId

	local allPlayerLines = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())
	local filteredLines : {Engine.Entity} = {}
	local allowedCarriers = commonParams.carrierFn()
	for __, lineEntity in ipairs(allPlayerLines) do
		if not allowedCarriers then
			break
		end

		if api.engine.entityExists(lineEntity) then
			if line_util.filterLine(allowedCarriers, lineEntity) then
				table.insert(filteredLines, lineEntity)
			end
		end
	end

	local selectedLines : {Engine.Entity} = {}
	for __, lineEAR in ipairs(commonParams.lineManagerStateRef:get().lineListEntitiesSelected) do
		table.insert(selectedLines, lineEAR.entity)
	end

	local function prepareContextBuilder() : LineUtil.ContextBuilder
		local ctx : LineUtil.ContextBuilder = line_util.MakeContextBuilder()
		ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))

		ctx.setComponentsFilter({
			api.type.ComponentType.LINE,
			api.type.ComponentType.STATION_GROUP,
			api.type.ComponentType.TRANSPORT_VEHICLE,
			api.type.ComponentType.VEHICLE_DEPOT,
			-- api.type.ComponentType.WAREHOUSE,
			-- api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.INDUSTRY,
			api.type.ComponentType.TOWN,
		})
		ctx.setUnselectableComponents({
			api.type.ComponentType.TOWN_BUILDING,
		})

		ctx.setHudMaster(manager_hud_util.LineVehicleManagerHudIconMaster)

		-- make it grayed out by default instead of missing
		if not commonParams.moreOptionsState:old() then
			ctx.iaHandler(
				"IA_SELECT",
				handlerNoOp,
				"compareHandlerNoOp",
				isEnabledReturnFalse,
				"compareIsEnabledFalse",
				_("Select")
			)
		end
		return ctx
	end

	local oldContextState, setContextState : (LineUtil.GuiContextState, function(LineUtil.GuiContextState))
		= line_util.useContextState(prepareContextBuilder, commonParams.updateIA)

	local selectedVehicles = getSelectedVehicleEntities(commonParams.vehicleManagerStateRef)

	-- update tooltips and the internal input actions. Return the new state for immediate use in case of click
	local function pickedEntity(details : LineUtil.SelectionDetails) : LineUtil.GuiContextState
		local ctx = prepareContextBuilder()

		if details and details.station and details.station.stationGroup and not entity_util.isOwnedByPlayerOrNotOwned(details.station.stationGroup) then
			return ctx.result
		end

		if not details or commonParams.getDisableHudInteraction() then
			setContextState(ctx.result)
			return ctx.result
		end

		local pickedStation = function(sgDetails : LineUtil.StationSelectionDetails)
			if commonParams.gameCtx.disableFeatures:get()["CreateNewLine"] then
				ctx.tooltip(manager_tooltips_util.CreateLineError, nil)
			else
				ctx.tooltip(manager_tooltips_util.CreateLine, sgDetails)
				ctx.iaClickOrSelect(
					function()
						commonParams.newLine(sgDetails, selectedVehicles)
						api.gui.sound.playRandomSoundEffect(lvmSFX.NewLine)
					end,
					{"compareNewLine", sgDetails},
					isEnabledReturnTrue,
					"compareIsEnabledTrue"
				)
			end
		end

		if details.line then
			ctx.tooltip(manager_tooltips_util.SelectLine, details.line)
			ctx.iaClickOrSelect(
				function()
					commonParams.selectLine(details.line, true, details.line.stopNumber)
					api.gui.sound.playRandomSoundEffect(guiCompSFX.ButtonClick) -- yes, we want this, as it should sound as in the list in 2d
				end,
				{"compareSelectLine", details.line.entity},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif details.station then
			pickedStation(details.station)
		elseif details.vehicleDepot then
			local vehiclesInDepot = commonParams.areAnyVehiclesInDepot(details.vehicleDepot.entity)
			details.vehicleDepot.removeFromList = commonParams.areAllDepotVehiclesInVehicleList(details.vehicleDepot.entity)

			if vehiclesInDepot then
				ctx.tooltip(manager_tooltips_util.AddOrRemoveDepotVehiclesToVehicleList, details.vehicleDepot)
			else
				ctx.tooltip(manager_tooltips_util.OpenDepot, details.vehicleDepot.entity)
			end

			ctx.iaClickOrSelect(
				function()
					if vehiclesInDepot then
						commonParams.selectOrDeselectDepot(details.vehicleDepot)
						commonParams.addOrRemoveDepotVehiclesToVehicleList(details.vehicleDepot)
					else
						commonParams.selectOrDeselectDepot(details.vehicleDepot)
						commonParams.openVehicleDepot(details.vehicleDepot.entity, nil)
					end
					api.gui.sound.playRandomSoundEffect(guiCompSFX.ButtonClick) -- yes, we want this, as it should sound as in the list in 2d
				end,
				{"compareOpenVehicleDepot", details.vehicleDepot},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif details.entity ~= nil then
			local entity = details.entity
			local vehicle = api.engine.getComponent(entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
			local depot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT) as Engine.Component.VehicleDepot
			local isDepot = depot and (#depot.inNodes + #depot.outNodes > 0)
			local warehouse = api.engine.getComponent(entity, api.type.ComponentType.WAREHOUSE) as Engine.Component.Warehouse
			local industry = api.engine.getComponent(entity, api.type.ComponentType.INDUSTRY) as Engine.Component.Industry
			local subconstruction = api.engine.getComponent(entity, api.type.ComponentType.SUBCONSTRUCTION) as Engine.Component.Subconstruction
			local construction = api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction

			if warehouse then
				ctx.tooltip(manager_tooltips_util.OpenWarehouse, entity)
				ctx.iaClickOrSelect(
					function()
						commonParams.openWarehouse(entity)
						api.gui.sound.playRandomSoundEffect(guiCompSFX.ButtonClick) -- yes, we want this, as it should sound as in the list in 2d
					end,
					{"compareOpenWarehouse", entity},
					isEnabledReturnTrue,
					"compareIsEnabledTrue"
				)
			elseif industry then
				if industry.construction then
					local industryCon = api.engine.getComponent(industry.construction, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
					pickFunctionForConstructionStation(industry.construction, industryCon, pickedStation)
				end
			elseif subconstruction then
				-- modular road station
				local conEntity = api.engine.system.streetConnectorSystem.getConstructionEntityForSubconstruction(details.entity)
				local con = api.engine.getComponent(conEntity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
				pickFunctionForConstructionStation(conEntity, con, pickedStation)
			elseif construction then
				-- modular rail station
				pickFunctionForConstructionStation(entity, construction, pickedStation)
			elseif not vehicle and not isDepot then
				local carriage = api.engine.getComponent(entity, api.type.ComponentType.CARRIAGE) as Engine.Component.Carriage
				if carriage then
					local info = api.engine.system.carriageListSystem.getVehicleInfo(entity)
					entity = info.owningVehicle
					vehicle = api.engine.getComponent(entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
				end
			end

			if vehicle then
				actionHandleVehicles(ctx, commonParams, entity)
			end
		end

		setContextState(ctx.result)
		return ctx.result
	end

	local lineVisualizations : {Builtin.Type.LineVisualization} = {}
	if allowedCarriers then
		for __, line in ipairs(filteredLines) do
			local lineVis = builtin.type.LineVisualization.new()
			lineVis.entity = line
			lineVis.transparency = 1.0
			table.insert(lineVisualizations, lineVis)
		end
	elseif #selectedLines > 0 then
		for __, line in ipairs(selectedLines) do
			local lineVis = builtin.type.LineVisualization.new()
			lineVis.entity = line
			lineVis.transparency = 1.0
			table.insert(lineVisualizations, lineVis)
		end
	else
		for __, line in ipairs(allPlayerLines) do
			local lineVis = builtin.type.LineVisualization.new()
			lineVis.entity = line
			lineVis.transparency = 1.0
			table.insert(lineVisualizations, lineVis)
		end
	end

	return line_util.makeLineActionDescriptor(
		commonParams,
		commonParams.gameCtx,
		commonParams.parent,
		commonParams.switchMode,
		lineVisualizations,
		nil,
		selectedVehicles,
		oldContextState,
		pickedEntity,
		commonParams.onProcessMouseEvent,
		true,
		false,
		false,
		commonParams.gamepadInputModeState:old(),
		commonParams.lvm2D3DModeState:old(),
		commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil,
		commonParams.preferLineOverTNEdgeState:old()
	)
end

local function makeActionSegmentMode(commonParams : CommonActionParams, insertAtStopNumber : integer) : TreeNodeId
	local lineEAR : EntityUtil.EntityAndRevision = nil
	if commonParams.lineState:old() and commonParams.lineState:old().entityAndRevision then
		lineEAR = commonParams.lineState:old().entityAndRevision
	end

	local lineEntity : Engine.Entity = nil
	local carriers : {Carrier : boolean} = {}

	if lineEAR and not entity_util.entityChanged0(lineEAR) then
		if entity_util.isOwnedByPlayer(lineEAR.entity) then
			lineEntity = lineEAR.entity
			carriers = line_util.getLineCarriers(commonParams.lineState:old())
		end
	end

	local water = carriers[api.type["enum"].Carrier.WATER]
	local air = carriers[api.type["enum"].Carrier.AIR]

	local function prepareContextBuilder() : LineUtil.ContextBuilder
		local ctx = line_util.MakeContextBuilder()
		ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
		ctx.setShowNavigableLayer(true)

		local insertAt = insertAtStopNumber and insertAtStopNumber or (commonParams.lineState:old() and #commonParams.lineState:old().path or 0)
		ctx.highlightLine(lineEntity, insertAt)
		ctx.showTerminalsForLine(lineEntity)
		ctx.setComponentsFilter({
			api.type.ComponentType.STATION_GROUP,
			api.type.ComponentType.LINE,
			api.type.ComponentType.WORLD,
			api.type.ComponentType.TRANSPORT_VEHICLE,
			api.type.ComponentType.TRANSPORT_NETWORK,
			api.type.ComponentType.VEHICLE_DEPOT,
			api.type.ComponentType.INDUSTRY,
			api.type.ComponentType.WAREHOUSE,
			-- api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TOWN,
		})
		ctx.setUnselectableComponents({
			api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TRANSPORT_VEHICLE,
		})

		ctx.setHudMaster(manager_hud_util.LineVehicleManagerHudIconMaster)

		if water or air then
			ctx.setTerrainCirclePolicy(app_util.getMouseOrGamepad("Never", "Always") as Builtin.TerrainCirclePolicy)
		end

		-- make it grayed out by default instead of missing
		if not commonParams.moreOptionsState:old() then
			ctx.iaHandler(
				"IA_SELECT",
				handlerNoOp,
				"compareHandlerNoOp",
				isEnabledReturnFalse,
				"compareIsEnabledFalse",
				_("Select")
			)
		end
		return ctx
	end

	local oldContextState, setContextState : (LineUtil.GuiContextState, function(LineUtil.GuiContextState))
		= line_util.useContextState(prepareContextBuilder, commonParams.updateIA)

	local selectedVehiclesState : ReactStateT<{Engine.Entity}> = engine_react_util.useStepState(function() : {Engine.Entity}
		return getSelectedVehicleEntities(commonParams.vehicleManagerStateRef)
	end)

	-- update tooltips and the internal input actions. Return the new state for immediate use in case of click
	local function pickedEntity(details : LineUtil.SelectionDetails) : LineUtil.GuiContextState
		local ctx = prepareContextBuilder()

		if details and details.station and details.station.stationGroup and not entity_util.isOwnedByPlayerOrNotOwned(details.station.stationGroup) then
			return ctx.result
		end

		local appendIaPreferWaypointIfLineSelected = function()
			if insertAtStopNumber ~= -1 then
				local startWithPreferWaypoint = api.gui.inputAction.modifierOnlyActionIsActive(commonParams.lvmButtonIAs.modifier3D_PreferWaypoint)
				if startWithPreferWaypoint == commonParams.preferLineOverTNEdgeState:old() then
					commonParams.preferLineOverTNEdgeState:set(not startWithPreferWaypoint)
				end

				ctx.iaHandlerExtended(
					commonParams.lvmButtonIAs.modifier3D_PreferWaypoint,
					function(invokeData : Gui.InputAction.InvokeData)
						local preferWaypoint : boolean
						if invokeData.status == api.gui.inputAction.InvokeData.Status.Triggered then
							preferWaypoint = true
						elseif invokeData.status == api.gui.inputAction.InvokeData.Status.EndReleased
							or invokeData.status == api.gui.inputAction.InvokeData.Status.EndAborted then
							preferWaypoint = false
						else
							return
						end

						commonParams.preferLineOverTNEdgeState:set(not preferWaypoint)
					end,
					"compareHandlerNoOp",
					isEnabledReturnTrue,
					"compareIsEnabledFalse",
					_("Prefer Waypoints")
				)
			end
		end

		commonParams.addStopHoveredStationState:set(nil)
		if not details or commonParams.getDisableHudInteraction() then
			appendIaPreferWaypointIfLineSelected()
			setContextState(ctx.result)
			return ctx.result
		end

		if not lineEntity then
			if details.station then
				ctx.tooltip(manager_tooltips_util.CreateLine, details.station)
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopInvalid"))
				ctx.iaClickOrSelect(
					function()
						commonParams.newLine(details.station, selectedVehiclesState:old())
						api.gui.sound.playRandomSoundEffect(lvmSFX.NewLine)
					end,
					{"compareNewLine", details.station},
					isEnabledReturnTrue,
					"compareIsEnabledTrue"
				)
			end
			appendIaPreferWaypointIfLineSelected()
			setContextState(ctx.result)
			return ctx.result
		end

		local allowAll = true
		if insertAtStopNumber == -1 then
			allowAll = false
			-- ctx.tooltip(manager_tooltips_util.LMSelectSectionFirst, nil)
			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
		end

		if details.line then
			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
			ctx.tooltip(manager_tooltips_util.LMSelectSection, details.line.stopNumber)
			ctx.highlightLine(details.line.entity, details.line.stopNumber)
			ctx.iaClickOrSelect(
				function()
					commonParams.switchMode({"SEGMENT", details.line.stopNumber})
				end,
				{"compareSelectStation", details.line},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		end
		if details.transportNetworkEdge then
			local baseEdge : Engine.Component.BaseEdge = api.engine.getComponent(details.transportNetworkEdge.snap.edgeId.entity, api.type.ComponentType.BASE_EDGE)
			if baseEdge then
				local transportNetwork : Engine.Component.TransportNetwork = api.engine.getComponent(details.transportNetworkEdge.snap.edgeId.entity, api.type.ComponentType.TRANSPORT_NETWORK)
				local snapPos = api.engine.util.transport.calcPosition(transportNetwork.edges[details.transportNetworkEdge.snap.edgeId.index + 1].geometry,
					details.transportNetworkEdge.snap.param)

				local minDistAndEntity : {number, Engine.Entity} = {math.huge, nil}
				for __, v in ipairs(baseEdge.objects) do
					if (v[2] as integer) < 2 then
						local edgeObjectEntity = v[1]

						local edgeObjectMil  : Engine.Component.ModelInstanceList = api.engine.getComponent(edgeObjectEntity, api.type.ComponentType.MODEL_INSTANCE_LIST)
						local edgeObjectPos = edgeObjectMil.fatInstances[1].transf:cols(3)
						local edgeObjectPos2 = api.type.Vec3f.new(edgeObjectPos.x, edgeObjectPos.y, edgeObjectPos.z)
						local dist = api.type.Vec3f.distance(edgeObjectPos2, snapPos)

						if dist < 10.0 and dist < minDistAndEntity[1] then
							minDistAndEntity = {dist, edgeObjectEntity}
						end
					end
				end
				if minDistAndEntity[2] then
					local stationGroupEntity = api.engine.system.stationGroupSystem.getStationGroup(minDistAndEntity[2])
					local stationGroup  : Engine.Component.StationGroup = api.engine.getComponent(stationGroupEntity, api.type.ComponentType.STATION_GROUP)
					local stationIndex1 = 1
					for k, v in ipairs(stationGroup.stations) do
						if v == minDistAndEntity[2] then
							stationIndex1 = k
						end
					end
					details.station = {
						stationGroup = stationGroupEntity,
						station = minDistAndEntity[2],
						stationIndex1 = stationIndex1,
						terminalIndex1 = 1,
						flatStationTerminalIndex1 = stationIndex1,
					}
				end
			end
		end

		local pickedStation = function(sDetails : LineUtil.StationSelectionDetails)
			if not allowAll then
				return
			end

			commonParams.addStopHoveredStationState:set(sDetails)
			ctx.tooltip(manager_tooltips_util.LMAddStop, sDetails)
			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopInvalid"))
			if sDetails.flatStationTerminalIndex1 ~= nil and sDetails.flatStationTerminalIndex1 >= 1 then
				ctx.highlightTerminal(sDetails)
			end
			ctx.iaClickOrSelect(
				function()
					commonParams.addStop(sDetails, insertAtStopNumber)
					api.gui.sound.playRandomSoundEffect(lvmSFX.AddStation)
				end,
				{"compareInsertStationAt", sDetails},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		end

		local station = details.stop and details.stop.station or details.station
		if station then
			pickedStation(station)
		elseif details.vehicleDepot then
			local depotAndLine : LineUtil.DepotAndLineParams = {
				depot = details.vehicleDepot.entity,
				line = lineEntity,
			}
			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
			ctx.tooltip(manager_tooltips_util.LMSelectDepotForLine, depotAndLine)
			ctx.iaClickOrSelect(
				function()
					commonParams.openVehicleDepot(details.vehicleDepot.entity, lineEntity)
					api.gui.sound.playRandomSoundEffect(guiCompSFX.ButtonClick) -- yes, we want this, as it should sound as in the list in 2d
				end,
				{"compareOpenVehicleDepot", details.vehicleDepot.entity, lineEntity},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif allowAll and (not water and not air and details.transportNetworkEdge) then
			local apiWaypoint = api.type.Waypoint.new()
			local edgePos = details.transportNetworkEdge.snap
			apiWaypoint.edgePos = edgePos

			if not commonParams.preferLineOverTNEdgeState:old() then

				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopInvalid"))
				ctx.tooltip(manager_tooltips_util.LMAddWaypoint, apiWaypoint)
				ctx.showSnapPreview(details.transportNetworkEdge)
				ctx.iaClickOrSelect(
					function()
						commonParams.addWaypoint(apiWaypoint, insertAtStopNumber)
						api.gui.sound.playRandomSoundEffect(lvmSFX.AddWaypoint)
					end,
					{"compareAddWaypoint", details.transportNetworkEdge},
					isEnabledReturnTrue,
					"compareIsEnabledTrue"
				)
			else
				ctx.tooltip(manager_tooltips_util.LMPromptWaypointModifier, apiWaypoint)
			end
		elseif allowAll and ((water or air) and details.world) then
			local apiWaypoint = api.type.Waypoint.new()
			apiWaypoint.pos = details.world.pos

			if not commonParams.preferLineOverTNEdgeState:old() and not details.world.water and not air then
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Invalid"))
				ctx.tooltip(manager_tooltips_util.LMWaterWaypointOnLand, apiWaypoint)
			elseif line_util.isWaypointTooClose(commonParams.lineState:old(), apiWaypoint.pos, nil) then
				-- do nothing
			elseif not commonParams.preferLineOverTNEdgeState:old() then
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopInvalid"))
				ctx.tooltip(manager_tooltips_util.LMAddWaypoint, apiWaypoint)
				ctx.iaClickOrSelect(
					function()
						commonParams.addWaypoint(apiWaypoint, insertAtStopNumber)
						api.gui.sound.playRandomSoundEffect(lvmSFX.AddWaypoint)
					end,
					{"compareAddWaypointWA", details.world},
					function() : boolean
						return not line_util.isWaypointTooClose(commonParams.lineState:old(), apiWaypoint.pos, nil)
					end,
					"compareIsWaypointTooClose"
				)
			elseif air or (water and details.world.water) then
				ctx.tooltip(manager_tooltips_util.LMPromptWaypointModifier, apiWaypoint)
			end
		elseif details.waypoint then
			local isEnabledWithStopProtectionCheck = function() : boolean
				return not line_util.isStopProtected(lineEntity, nil, commonParams.gameCtx.filters:get().protectedEntities)
			end


			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopInvalid"))
			ctx.tooltip(manager_tooltips_util.LMMoveWaypoint, details.waypoint)
			ctx.highlightWaypoint(details.waypoint)
			ctx.iaClickOrSelect(
				function()
					commonParams.switchMode({"MOVE_WAYPOINT", details.waypoint})
				end,
				{"compareMoveWaypoint", details.waypoint},
				isEnabledWithStopProtectionCheck,
				"compareIsEnabledTrue"
			)
		elseif details.entity ~= nil then
			if water then
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Invalid"))
			else
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
			end
			local entity = details.entity
			local vehicle = api.engine.getComponent(details.entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
			local depot = api.engine.getComponent(details.entity, api.type.ComponentType.VEHICLE_DEPOT) as Engine.Component.VehicleDepot
			local isDepot = depot and (#depot.inNodes + #depot.outNodes > 0)
			local industry = api.engine.getComponent(entity, api.type.ComponentType.INDUSTRY) as Engine.Component.Industry
			local subconstruction = api.engine.getComponent(entity, api.type.ComponentType.SUBCONSTRUCTION) as Engine.Component.Subconstruction
			local construction = api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction

			if industry then
				if industry.construction then
					local industryCon = api.engine.getComponent(industry.construction, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
					pickFunctionForConstructionStation(industry.construction, industryCon, pickedStation)
				end
			elseif subconstruction then
				-- modular road station
				local conEntity = api.engine.system.streetConnectorSystem.getConstructionEntityForSubconstruction(details.entity)
				local con = api.engine.getComponent(conEntity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
				pickFunctionForConstructionStation(conEntity, con, pickedStation)
			elseif construction then
				-- modular rail station
				pickFunctionForConstructionStation(entity, construction, pickedStation)
			elseif not vehicle and not isDepot then
				local carriage = api.engine.getComponent(details.entity, api.type.ComponentType.CARRIAGE) as Engine.Component.Carriage
				if carriage then
					ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
					local info = api.engine.system.carriageListSystem.getVehicleInfo(entity)
					entity = info.owningVehicle
					vehicle = api.engine.getComponent(entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
				end
			else
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
			end

			if vehicle then
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
				actionHandleVehicles(ctx, commonParams, entity)
			end
		end

		appendIaPreferWaypointIfLineSelected()
		setContextState(ctx.result)
		return ctx.result
	end

	local lineVisualizations : {Builtin.Type.LineVisualization} = {}
	if lineEntity then
		local lineVis = builtin.type.LineVisualization.new()
		lineVis.entity = lineEntity
		lineVis.transparency = 1.0
		table.insert(lineVisualizations, lineVis)
	end

	return line_util.makeLineActionDescriptor(
		commonParams,
		commonParams.gameCtx,
		commonParams.parent,
		commonParams.switchMode,
		lineVisualizations,
		nil,
		selectedVehiclesState:old(),
		oldContextState,
		pickedEntity,
		commonParams.onProcessMouseEvent,
		true,
		true,
		true,
		commonParams.gamepadInputModeState:old(),
		commonParams.lvm2D3DModeState:old(),
		commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil,
		commonParams.preferLineOverTNEdgeState:old()
	)
end

local function makeActionMoveStopMode(commonParams : CommonActionParams, movingStop : LineUtil.StopSelectionDetails) : TreeNodeId
	local lineEntity : Engine.Entity = nil
	if commonParams.lineState:old() and commonParams.lineState:old().entityAndRevision then
		lineEntity = commonParams.lineState:old().entityAndRevision.entity
	end

	local function prepareContextBuilder() : LineUtil.ContextBuilder
		local ctx = line_util.MakeContextBuilder()

		ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Grab"))
		ctx.setShowNavigableLayer(true)

		if movingStop.station then
			ctx.highlightTerminal(movingStop.station)
		end

		ctx.setHudMaster(manager_hud_util.LineVehicleManagerHudIconMaster)

		ctx.setComponentsFilter({
			api.type.ComponentType.STATION_GROUP,
			api.type.ComponentType.LINE,
			api.type.ComponentType.WORLD,
			api.type.ComponentType.SIGNAL_LIST,
			api.type.ComponentType.TRANSPORT_VEHICLE,
			api.type.ComponentType.INDUSTRY,
			api.type.ComponentType.WAREHOUSE,
			-- api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TOWN,
			-- api.type.ComponentType.TRANSPORT_NETWORK,
		})
		ctx.setUnselectableComponents({
			api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TRANSPORT_VEHICLE,
		})

		-- make it grayed out by default instead of missing
		if not commonParams.moreOptionsState:old() then
			ctx.iaHandler(
				"IA_SELECT",
				handlerNoOp,
				"compareHandlerNoOp",
				isEnabledReturnFalse,
				"compareIsEnabledFalse",
				_("Select")
			)

			if not commonParams.gamepadInputModeState:old() then
				ctx.iaHandler(
					"IA_MENU_BACK",
					function()
						commonParams.switchMode({"SEGMENT", nil})
					end,
					"compareCancelMoveStop",
					isEnabledReturnTrue,
					"compareIsEnabledReturnTrue",
					_("Cancel Moving Stop")
				)
			end
		end

		return ctx
	end

	local oldContextState, setContextState : (LineUtil.GuiContextState, function(LineUtil.GuiContextState))
		= line_util.useContextState(prepareContextBuilder, commonParams.updateIA)

	local selectedVehicles = getSelectedVehicleEntities(commonParams.vehicleManagerStateRef)

	-- update tooltips and the internal input actions. Return the new state for immediate use in case of click
	local function pickedEntity(details : LineUtil.SelectionDetails) : LineUtil.GuiContextState
		local ctx = prepareContextBuilder()

		if details and details.station and details.station.stationGroup and not entity_util.isOwnedByPlayerOrNotOwned(details.station.stationGroup) then
			return ctx.result
		end

		if not details or commonParams.getDisableHudInteraction() then
			setContextState(ctx.result)
			return ctx.result
		end

		if movingStop.station then
			ctx.highlightTerminal(movingStop.station)
		end

		if not commonParams.getDisableHudInteraction() then
			ctx.tooltip(manager_tooltips_util.LMDefaultMoveStop, nil)
		end

		local pickedStation = function(sDetails : LineUtil.StationSelectionDetails)
			local isEnabledWithStopProtectionCheck = function() : boolean
				return not line_util.isStopProtected(lineEntity, sDetails.stationGroup, commonParams.gameCtx.filters:get().protectedEntities)
			end

			if isEnabledWithStopProtectionCheck() then
				ctx.tooltip(manager_tooltips_util.LMMoveStopTarget, sDetails)
			else
				ctx.tooltip(manager_tooltips_util.LMMoveStopTargetError, sDetails)
			end

			if sDetails.flatStationTerminalIndex1 ~= nil and sDetails.flatStationTerminalIndex1 >= 1 then
				ctx.highlightTerminal(sDetails)
			end
			ctx.iaClickOrSelect(
				function()
					if not isEnabledWithStopProtectionCheck() then
						return
					end
					commonParams.moveStop(movingStop, sDetails)
				end,
				{"compareMoveStation", sDetails},
				isEnabledWithStopProtectionCheck,
				"compareIsEnabledTrue"
			)
		end

		local station = details.stop and details.stop.station or details.station

		if station then
			pickedStation(station)
		elseif details.entity ~= nil then
			local entity = details.entity
			local industry = api.engine.getComponent(entity, api.type.ComponentType.INDUSTRY) as Engine.Component.Industry
			local subconstruction = api.engine.getComponent(entity, api.type.ComponentType.SUBCONSTRUCTION) as Engine.Component.Subconstruction
			local construction = api.engine.getComponent(entity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction

			if industry then
				if industry.construction then
					local industryCon = api.engine.getComponent(industry.construction, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
					pickFunctionForConstructionStation(industry.construction, industryCon, pickedStation)
				end
			elseif subconstruction then
				-- modular road station
				local conEntity = api.engine.system.streetConnectorSystem.getConstructionEntityForSubconstruction(details.entity)
				local con = api.engine.getComponent(conEntity, api.type.ComponentType.CONSTRUCTION) as Engine.Component.Construction
				pickFunctionForConstructionStation(conEntity, con, pickedStation)
			elseif construction then
				-- modular rail station
				pickFunctionForConstructionStation(entity, construction, pickedStation)
			end
		end

		setContextState(ctx.result)
		return ctx.result
	end

	local lineVisualizations : {Builtin.Type.LineVisualization} = {}
	if lineEntity then
		local lineVis = builtin.type.LineVisualization.new()
		lineVis.entity = lineEntity
		lineVis.transparency = 1.0
		table.insert(lineVisualizations, lineVis)
	end

	return line_util.makeLineActionDescriptor(
		commonParams,
		commonParams.gameCtx,
		commonParams.parent,
		commonParams.switchMode,
		lineVisualizations,
		nil,
		selectedVehicles,
		oldContextState,
		pickedEntity,
		commonParams.onProcessMouseEvent,
		false,
		true,
		false,
		commonParams.gamepadInputModeState:old(),
		commonParams.lvm2D3DModeState:old(),
		commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil,
		commonParams.preferLineOverTNEdgeState:old()
	)
end

local function makeActionMoveWaypointMode(commonParams : CommonActionParams, movingWaypoint : LineUtil.WaypointSelectionDetails) : TreeNodeId
	local lineEAR : EntityUtil.EntityAndRevision = nil
	if commonParams.lineState:old() and commonParams.lineState:old().entityAndRevision then
		lineEAR = commonParams.lineState:old().entityAndRevision
	end

	local lineEntity : Engine.Entity = nil
	local carriers : {Carrier : boolean} = {}

	if lineEAR and not entity_util.entityChanged0(lineEAR) then
		if entity_util.isOwnedByPlayer(lineEAR.entity) then
			lineEntity = lineEAR.entity
			carriers = line_util.getLineCarriers(commonParams.lineState:old())
		end
	end

	local water = carriers[api.type["enum"].Carrier.WATER]
	local air = carriers[api.type["enum"].Carrier.AIR]

	local function prepareContextBuilder() : LineUtil.ContextBuilder
		local ctx = line_util.MakeContextBuilder()
		ctx.setShowNavigableLayer(true)
		ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Grab"))
		ctx.showTerminalsForLine(lineEntity)

		ctx.highlightLine(movingWaypoint.line, movingWaypoint.stopNumber)
		ctx.highlightWaypoint(movingWaypoint)

		ctx.setHudMaster(manager_hud_util.LineVehicleManagerHudIconMaster)

		ctx.setComponentsFilter({
			api.type.ComponentType.STATION_GROUP,
			api.type.ComponentType.SIGNAL_LIST,
			api.type.ComponentType.LINE,
			api.type.ComponentType.WORLD,
			api.type.ComponentType.TRANSPORT_VEHICLE,
			api.type.ComponentType.TRANSPORT_NETWORK,
			api.type.ComponentType.INDUSTRY,
			api.type.ComponentType.WAREHOUSE,
			-- api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TOWN,
		})
		ctx.setUnselectableComponents({
			api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TRANSPORT_VEHICLE,
		})
		if water or air then
			ctx.setTerrainCirclePolicy("Always")
		end

		-- make it grayed out by default instead of missing
		if not commonParams.moreOptionsState:old() then
			ctx.iaHandler(
				"IA_SELECT",
				handlerNoOp,
				"compareHandlerNoOp",
				isEnabledReturnFalse,
				"compareIsEnabledFalse",
				_("Select")
			)
			if not commonParams.gamepadInputModeState:old() then
				ctx.iaHandler(
					"IA_MENU_BACK",
					function()
						commonParams.switchMode({"SEGMENT", nil})
					end,
					"compareCancelMoveWaypoint",
					isEnabledReturnTrue,
					"compareIsEnabledReturnTrue",
					_("Cancel Moving Waypoint")
				)
			end
		end

		return ctx
	end

	local oldContextState, setContextState : (LineUtil.GuiContextState, function(LineUtil.GuiContextState))
		= line_util.useContextState(prepareContextBuilder, commonParams.updateIA)

	local selectedVehicles = getSelectedVehicleEntities(commonParams.vehicleManagerStateRef)

	-- update tooltips and the internal input actions. Return the new state for immediate use in case of click
	local function pickedEntity(details : LineUtil.SelectionDetails) : LineUtil.GuiContextState
		local ctx = prepareContextBuilder()

		if not details or commonParams.getDisableHudInteraction() then
			setContextState(ctx.result)
			return ctx.result
		end

		if not commonParams.getDisableHudInteraction() then
			ctx.tooltip(manager_tooltips_util.LMDefaultMoveWaypoint, nil)
		end

		if not water and not air and details.transportNetworkEdge then
			local edgePos = details.transportNetworkEdge.snap
			local apiWaypoint = api.type.Waypoint.new()
			apiWaypoint.edgePos = edgePos
			ctx.tooltip(manager_tooltips_util.LMMoveWaypointTarget, apiWaypoint)
			ctx.showSnapPreview(details.transportNetworkEdge)
			ctx.iaClickOrSelect(
				function()
					commonParams.moveWaypoint(movingWaypoint, apiWaypoint)
				end,
				{"compareMoveWaypoint", details.transportNetworkEdge},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif (water or air) and details.world then
			local apiWaypoint = api.type.Waypoint.new()
			apiWaypoint.pos = details.world.pos
			if not details.world.water and not air then
				ctx.tooltip(manager_tooltips_util.LMWaterWaypointOnLand, apiWaypoint)
			elseif line_util.isWaypointTooClose(commonParams.lineState:old(), apiWaypoint.pos, movingWaypoint.stopNumber) then
				ctx.tooltip(manager_tooltips_util.LMWaypointTooClose, apiWaypoint)
				ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Invalid"))
			else
				ctx.tooltip(manager_tooltips_util.LMMoveWaypointTarget, apiWaypoint)
				ctx.iaClickOrSelect(
					function()
						commonParams.moveWaypoint(movingWaypoint, apiWaypoint)
					end,
					{"compareMoveWaypointWA", apiWaypoint},
					function() : boolean
						return not line_util.isWaypointTooClose(commonParams.lineState:old(), apiWaypoint.pos, nil)
					end,
					{"compareIsEnabledMoveWaypointWA", apiWaypoint}
				)
			end
		end

		setContextState(ctx.result)
		return ctx.result
	end

	local lineVisualizations : {Builtin.Type.LineVisualization} = {}
	if lineEntity then
		local lineVis = builtin.type.LineVisualization.new()
		lineVis.entity = lineEntity
		lineVis.transparency = 1.0
		table.insert(lineVisualizations, lineVis)
	end

	return line_util.makeLineActionDescriptor(
		commonParams,
		commonParams.gameCtx,
		commonParams.parent,
		commonParams.switchMode,
		lineVisualizations,
		nil,
		selectedVehicles,
		oldContextState,
		pickedEntity,
		commonParams.onProcessMouseEvent,
		false,
		true,
		false,
		commonParams.gamepadInputModeState:old(),
		commonParams.lvm2D3DModeState:old(),
		commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil,
		commonParams.preferLineOverTNEdgeState:old()
	)
end

local function makeActionSendVehiclesMode(commonParams : CommonActionParams) : TreeNodeId

	local selectedVehicles = getSelectedVehicleEntities(commonParams.vehicleManagerStateRef)

	local filteredLines = makeFilteredLinesForSendToLine(selectedVehicles, false)
	local hoveredLine = commonParams.sendToLineHoveredLineState:old()

	local function prepareContextBuilder() : LineUtil.ContextBuilder
		local ctx = line_util.MakeContextBuilder()

		ctx.setComponentsFilter({
			api.type.ComponentType.STATION_GROUP,
			api.type.ComponentType.LINE,
			-- api.type.ComponentType.TRANSPORT_VEHICLE,
			api.type.ComponentType.VEHICLE_DEPOT,
			api.type.ComponentType.INDUSTRY,
			-- api.type.ComponentType.WAREHOUSE,
			-- api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TOWN,
			-- api.type.ComponentType.TRANSPORT_NETWORK,
		})
		ctx.setUnselectableComponents({
			api.type.ComponentType.TOWN_BUILDING,
			api.type.ComponentType.TRANSPORT_VEHICLE,
		})

		ctx.setHudMaster(manager_hud_util.LineVehicleManagerHudIconMaster)

		if not commonParams.getDisableHudInteraction() then
			ctx.tooltip(manager_tooltips_util.VMDefault, #selectedVehicles)
		end

		-- make it grayed out by default instead of missing
		if not commonParams.moreOptionsState:old() then
			ctx.iaHandler(
				"IA_SELECT",
				handlerNoOp,
				"compareHandlerNoOp",
				isEnabledReturnFalse,
				"compareIsEnabledFalse",
				_("Select")
			)
		end
		return ctx
	end

	local oldContextState, setContextState : (LineUtil.GuiContextState, function(LineUtil.GuiContextState))
		= line_util.useContextState(prepareContextBuilder, commonParams.updateIA)

	-- update tooltips and the internal input actions. Return the new state for immediate use in case of click
	local function pickedEntity(details : LineUtil.SelectionDetails) : LineUtil.GuiContextState
		local ctx = prepareContextBuilder()

		if details and details.station and details.station.stationGroup and not entity_util.isOwnedByPlayerOrNotOwned(details.station.stationGroup) then
			return ctx.result
		end

		if not details or commonParams.getDisableHudInteraction() then
			setContextState(ctx.result)
			return ctx.result
		end

		ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("AddStopValid"))

		if details.line then
			local sendToLineParams : ManagerTooltipsUtil.SendToLineOrDepotParams = {
				numVehicles = #selectedVehicles,
				lineOrDepotEntity = details.line.entity,
			}
			ctx.tooltip(manager_tooltips_util.VMSendToLine, sendToLineParams)
			ctx.iaClickOrSelect(
				function()
					commonParams.sendVehiclesToLine(selectedVehicles, details.line.entity)
				end,
				{"compareSendVehiclesToLine", details.line},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif details.vehicleDepot then
			local sendToDepotParams : ManagerTooltipsUtil.SendToLineOrDepotParams = {
				numVehicles = #selectedVehicles,
				lineOrDepotEntity = details.vehicleDepot.entity,
			}
			ctx.tooltip(manager_tooltips_util.VMSendToDepot, sendToDepotParams)
			ctx.iaClickOrSelect(
				function()
					commonParams.sendVehiclesToDepot(selectedVehicles, details.vehicleDepot.entity)
					api.gui.sound.playRandomSoundEffect(lvmSFX.SendToDepot)
				end,
				{"compareSendVehiclesToDepot", details.vehicleDepot.entity},
				isEnabledReturnTrue,
				"compareIsEnabledTrue"
			)
		elseif details.station then
			if commonParams.gameCtx.disableFeatures:get()["CreateNewLine"] then
				ctx.tooltip(manager_tooltips_util.CreateLineError, nil)
			else
				ctx.tooltip(manager_tooltips_util.VMSendToNewLine, #selectedVehicles)
				ctx.iaClickOrSelect(
					function()
						commonParams.newLine(details.station, selectedVehicles)
						api.gui.sound.playRandomSoundEffect(lvmSFX.NewLine)
					end,
					{"compareNewLine", details.station},
					isEnabledReturnTrue,
					"compareIsEnabledTrue"
				)
			end
		elseif details.stop then
			-- ctx.tooltip(manager_tooltips_util.Debug, "Does this case ever Happen (send to line for stop)")
		else
			ctx.setMouseCursor(gui_react_util.getMouseCursorInteger("Default"))
		end

		setContextState(ctx.result)
		return ctx.result
	end

	local lineVisualizations : {Builtin.Type.LineVisualization} = {}
	for __, line in ipairs(filteredLines) do
		local transparency = 1.0
		if hoveredLine then
			transparency = line == hoveredLine and 1.0 or 0.4
		end
		local lineVis = builtin.type.LineVisualization.new()
		lineVis.entity = line
		lineVis.transparency = transparency
		table.insert(lineVisualizations, lineVis)
	end

	return line_util.makeLineActionDescriptor(
		commonParams,
		commonParams.gameCtx,
		commonParams.parent,
		commonParams.switchMode,
		lineVisualizations,
		hoveredLine,
		selectedVehicles,
		oldContextState,
		pickedEntity,
		commonParams.onProcessMouseEvent,
		true,
		false,
		false,
		commonParams.gamepadInputModeState:old(),
		commonParams.lvm2D3DModeState:old(),
		commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil,
		commonParams.preferLineOverTNEdgeState:old()
	)
end

local makeLineManagerState = function(
	lineEntitiesInitialSelection : {EntityUtil.EntityAndRevision},
	depotEntitiesInitialSelection : {EntityUtil.EntityAndRevision},
	justDeletedLine : Engine.Entity
) : LineManagerState
	local lineListEntitiesSelected : {EntityUtil.EntityAndRevision} = {}
	local lineListEntitiesUnselected : {EntityUtil.EntityAndRevision} = {}

	local allPlayerLines = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())

	for __, lineEntity in ipairs(allPlayerLines) do
		if lineEntity ~= justDeletedLine then
			local found = false
			for __, lineEARSelected in ipairs(lineEntitiesInitialSelection) do
				if lineEntity == lineEARSelected.entity then
					table.insert(lineListEntitiesSelected, entity_util.makeEntityAndRevision(lineEntity))
					found = true
					break
				end
			end
			if not found then
				table.insert(lineListEntitiesUnselected, entity_util.makeEntityAndRevision(lineEntity))
			end
		end
	end

	local depotListEntitiesSelected : {EntityUtil.EntityAndRevision} = {}
	local depotListEntitiesUnselected : {EntityUtil.EntityAndRevision} = {}

	local allPlayerDepotEntities : {Engine.Entity} = api.engine.getEntitiesWithComponent(api.type.ComponentType.VEHICLE_DEPOT, {
		requireOwnedByPlayer = api.engine.util.getPlayer(),
		filterFn = function(entity : Engine.Entity) : boolean
			local vehicleDepot : Engine.Component.VehicleDepot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT)
			return vehicleDepot and (#vehicleDepot.inNodes > 0 or #vehicleDepot.outNodes > 0)
		end,
	})

	for __, depotEntity in ipairs(allPlayerDepotEntities) do
		local found = false
		for __, depotEARSelected in ipairs(depotEntitiesInitialSelection) do
			if depotEntity == depotEARSelected.entity then
				table.insert(depotListEntitiesSelected, entity_util.makeEntityAndRevision(depotEntity))
				found = true
				break
			end
		end
		if not found then
			table.insert(depotListEntitiesUnselected, entity_util.makeEntityAndRevision(depotEntity))
		end
	end
	return {
		lineListEntitiesSelected = lineListEntitiesSelected,
		lineListEntitiesUnselected = lineListEntitiesUnselected,
		depotListEntitiesSelected = depotListEntitiesSelected,
		depotListEntitiesUnselected = depotListEntitiesUnselected,
	}
end

local function selectSingleLineAndOverrideSelectionOnClick(
	vehicleManagerApi : VehicleManagerApi,
	lineAndDepotListApi : LineAndDepotListApi,
	creatingNewLineState : ReactStateT<boolean>,
	entity : Engine.Entity,
	focusLine : boolean,
	selectSegment : integer
)

	creatingNewLineState:set(false)

	lineAndDepotListApi.deselectAllLinesAndDepots()
	lineAndDepotListApi.selectLine(entity, true, selectSegment)

	if focusLine then
		api.gui.camera.followEntity(entity, true)
	end

	if vehicleManagerApi then
		vehicleManagerApi.clearVehicleList()

		local vehiclesOnLine = api.engine.system.transportVehicleSystem.getLineVehicles(entity)
		vehicleManagerApi.addVehiclesToVehicleListAndSelect(vehiclesOnLine)
	end
end

local function selectSingleDepotAndOverrideSelectionOnClick(
	vehicleManagerApi : VehicleManagerApi,
	lineAndDepotListApi : LineAndDepotListApi,
	creatingNewLineState : ReactStateT<boolean>,
	entity : Engine.Entity,
	focusDepot : boolean
)

	creatingNewLineState:set(false)

	lineAndDepotListApi.deselectAllLinesAndDepots()
	lineAndDepotListApi.selectDepot(entity, true)

	if focusDepot then
		api.gui.camera.followEntity(entity, true)
	end

	if vehicleManagerApi then
		vehicleManagerApi.clearVehicleList()

		local vehiclesInDepot = api.engine.system.transportVehicleSystem.getDepotVehicles(entity)
		vehicleManagerApi.addVehiclesToVehicleListAndSelect(vehiclesInDepot)
	end
end

local function openDepotWindow(
	gameCtx : GameContext,
	depotEntity : Engine.Entity,
	lineToSendEntity : Engine.Entity,
	vehicleStoreOpenForDepotAndLineRef : ReactRefT<VehicleStoreOpenInfo>,
	openVehicleDepot : function(depotEntity : Engine.Entity, lineEntity : Engine.Entity)
)
	local openDepotAndLine = vehicleStoreOpenForDepotAndLineRef:get()

	local wc = gameCtx.windowContainer:get():getApi()
	wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
	vehicleStoreOpenForDepotAndLineRef:set(nil)

	if (depotEntity ~= nil and depotEntity >= 0)
		and ((not openDepotAndLine)
		or (
			(openDepotAndLine.depot ~= depotEntity)
			or (openDepotAndLine.line ~= lineToSendEntity)
		)) then

		openVehicleDepot(depotEntity, lineToSendEntity)
	end
end

local makeVehicleManagerState = function(
	vehicleEntitiesInitialSelection : {EntityUtil.EntityAndRevision},
	vehicleEntitiesInitialUnselection : {EntityUtil.EntityAndRevision}
) : VehicleManagerState
	local vehicleListEntitiesSelected : {EntityUtil.EntityAndRevision} = {}
	local vehicleListEntitiesUnselected : {EntityUtil.EntityAndRevision} = {}
	local carriersMap = {}

	for __, vEAR in ipairs(vehicleEntitiesInitialSelection) do
		if not entity_util.entityChanged0(vEAR) then
			if entity_util.isOwnedByPlayer(vEAR.entity) then
				local tv : Engine.Component.TransportVehicle = api.engine.getComponent(vEAR.entity, api.type.ComponentType.TRANSPORT_VEHICLE)

				if tv then
					table.insert(vehicleListEntitiesSelected, vEAR)
					carriersMap[tv.carrier] = true
				end
			end
		end
	end
	for __, vEAR in ipairs(vehicleEntitiesInitialUnselection) do
		if not entity_util.entityChanged0(vEAR) then
			if entity_util.isOwnedByPlayer(vEAR.entity) then
				local tv : Engine.Component.TransportVehicle = api.engine.getComponent(vEAR.entity, api.type.ComponentType.TRANSPORT_VEHICLE)

				if tv then
					table.insert(vehicleListEntitiesUnselected, vEAR)
					carriersMap[tv.carrier] = true
				end
			end
		end
	end

	local carriers = {}
	for carrier, __ in pairs(carriersMap) do
		table.insert(carriers, carrier)
	end

	return {
		vehicleListEntitiesSelected = vehicleListEntitiesSelected,
		vehicleListEntitiesUnselected = vehicleListEntitiesUnselected,
		carriers = carriers,
	}
end

local makeLineProblemsTooltip = function(problemCount : integer) : string
	if problemCount > 0 then
		return lang_util.format(
			nGetText(
				"{count} Line With Problems",
				"{count} Lines With Problems",
				problemCount),
			{
				count = lang_util.formatInt(problemCount),
			}
		)
	end
	return nil
end

local makeVehicleProblemsTooltip = function(problemCount : integer) : string
	if problemCount > 0 then
		return lang_util.format(
			nGetText(
				"{count} Vehicle With Problems",
				"{count} Vehicles With Problems",
				problemCount),
			{
				count = lang_util.formatInt(problemCount),
			}
		)
	end
	return nil
end

local makeCheckboxWithProblemCount = function(
	checkbox : TreeNodeId,
	problemCount : integer
) : TreeNodeId
	return builtin.Component{
		layout = builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					h = -1,
					v = -1,
					item = checkbox,
				},
				problemCount > 0 and builtin.FloatingLayoutChild{
					h = 1.0,
					v = 0.0,
					overflowMode = builtin.type.FloatingLayoutOverflowMode.Overflow,
					item = builtin.TextView{
						meta = {
							class = "overflow-mode, font-scale-annotation, line-problem-count",
							mouseTransparent = true,
						},
						text = lang_util.formatInt(problemCount),
					},
				} or nil,
			},
		}
	}
end

local record LineManagerActionButtonsParams
	filterState : ReactStateT<StatisticsReactUtil.FilterState>
	filterConfigState : ReactStateT<LVMFilterConfig>

	commonParams : CommonActionParams

	scrollToRow : function(entity : Engine.Entity)
end

local LineManagerActionButtons = react.RegisterRecipe("LineManagerActionButtons", function(params : LineManagerActionButtonsParams) : TreeNodeId
	local defaultColorsRef = react.useRefLazy(function() : DefaultColors return api.gui.genericRep.get(api.gui.genericRep.find("::/gui/main/default_colors.gres")).data as DefaultColors end)

	local firstRef = react.useNodeRef()
	local focusedFirstState : ReactStateT<boolean> = react.useState(false)
	local focusedLastState : ReactStateT<boolean> = react.useState(false)
	if focusedFirstState:old() then
		react.useInputAction("IA_NAV_LEFT", react.iaHandler(function()
			if params.commonParams.btnNewLineRef:get() then
				params.commonParams.btnNewLineRef:get():focus()
			end
		end))
	end
	if focusedLastState:old() then
		react.useInputAction("IA_NAV_RIGHT", react.iaHandler(function()
			if firstRef:get() then
				firstRef:get():focus()
			end
		end))
	end

	local lineSelectionValue = params.commonParams.countDataState:old().selectionRatioLines
	if lineSelectionValue ~= 0 and lineSelectionValue ~= 1 then
		lineSelectionValue = -1
	end
	local depotSelectionValue = params.commonParams.countDataState:old().selectionRatioDepots
	if depotSelectionValue ~= 0 and depotSelectionValue ~= 1 then
		depotSelectionValue = -1
	end
	local totalLinesCount = params.commonParams.countDataState:old().countLinesSelected + params.commonParams.countDataState:old().countLinesUnselected
	local totalDepotsCount = params.commonParams.countDataState:old().countDepotsSelected + params.commonParams.countDataState:old().countDepotsUnselected

	local actionListChildren = {}

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return params.commonParams.moreOptionsState:old()
	end)

	local selectDepots = true -- otherwise only select lines
	local checkboxValue : number = 0
	local totalCount = 0
	if selectDepots then
		totalCount = totalLinesCount + totalDepotsCount
		if totalLinesCount == 0 then
			checkboxValue = depotSelectionValue
		elseif totalDepotsCount == 0 then
			checkboxValue = lineSelectionValue
		else
			if lineSelectionValue == 0 and depotSelectionValue == 0 then
				checkboxValue = 0
			elseif lineSelectionValue == 1 and depotSelectionValue == 1 then
				checkboxValue = 1
			else
				checkboxValue = -1
			end
		end
	else
		totalCount = totalLinesCount
		checkboxValue = lineSelectionValue
	end
	local allLinesCheckbox = builtin.CheckBox(react.ref(firstRef), {
		meta = {
			disableFocusableRecursive = moreOptionsLocalState:old(),
			enabled = totalCount > 0,
			tooltip = makeLineProblemsTooltip(params.commonParams.countDataState:old().countLinesWithProblems),
			onFocusChange = function(focus : boolean)
				focusedFirstState:set(focus)
			end,
		},
		onValueChange = function(__val : integer)
			local vmApi : VehicleManagerApi = nil
			if params.commonParams.vehicleManagerRef then
				vmApi = params.commonParams.vehicleManagerRef:get():getApi()
			end

			local onlyVisible = params.filterState:old().onlyVisible
			local searchString = params.filterState:old().search or ""
			local allowedCarriers : {Carrier} = params.commonParams.carrierFn()

			local addToSelection = false
			if selectDepots then
				addToSelection = (lineSelectionValue ~= 1) or (depotSelectionValue ~= 1)
			else
				addToSelection = lineSelectionValue ~= 1
			end
			if addToSelection then
				local allPlayerLineEARsFiltered : {EntityUtil.EntityAndRevision} = {}
				for __, playerLine in ipairs(api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())) do
					if not onlyVisible or api.gui.byEntity.isLineEmptyOrVisible(playerLine) then
						if not allowedCarriers or api.engine.util.line.isLineCompatibleWithAnyCarrier(playerLine, allowedCarriers, false) then
							local lineName = api.engine.util.getEntityName(playerLine)
							if searchString == "" or lang_util.stringContains(lineName, searchString) then
								table.insert(allPlayerLineEARsFiltered, entity_util.makeEntityAndRevision(playerLine))
							end
						end
					end
				end

				local allPlayerDepotEARsFiltered : {EntityUtil.EntityAndRevision} = {}
				for __, depotEAR in ipairs(params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected) do
					local depot : Engine.Component.VehicleDepot = api.engine.getComponent(depotEAR.entity, api.type.ComponentType.VEHICLE_DEPOT)

					if not onlyVisible or api.gui.byEntity.isVisible(depotEAR.entity) then
						if not allowedCarriers or statistics_react_util.areCarriersContainedInTransportModes(depot.transportModes, allowedCarriers) then
							local depotName = api.engine.util.getEntityName(depotEAR.entity)
							if searchString == "" or lang_util.stringContains(depotName, searchString) then
								table.insert(allPlayerDepotEARsFiltered, depotEAR);
							end
						end
					end
				end
				for __, depotEAR in ipairs(params.commonParams.lineManagerStateRef:get().depotListEntitiesUnselected) do
					local depot : Engine.Component.VehicleDepot = api.engine.getComponent(depotEAR.entity, api.type.ComponentType.VEHICLE_DEPOT)

					if not onlyVisible or api.gui.byEntity.isVisible(depotEAR.entity) then
						if not allowedCarriers or statistics_react_util.areCarriersContainedInTransportModes(depot.transportModes, allowedCarriers) then
							local depotName = api.engine.util.getEntityName(depotEAR.entity)
							if searchString == "" or lang_util.stringContains(depotName, searchString) then
								table.insert(allPlayerDepotEARsFiltered, depotEAR);
							end
						end
					end
				end

				params.commonParams.lineManagerStateRef:set(makeLineManagerState(
					allPlayerLineEARsFiltered,
					selectDepots and allPlayerDepotEARsFiltered
						or params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected,
					nil
				))
			else
				params.commonParams.lineManagerStateRef:set(makeLineManagerState({}, {}, nil))
				vmApi.clearVehicleList()
			end

			local addOrRemoveVehiclesForLine = function(lineEntity : Engine.Entity, add : boolean)
				local lineComp : Engine.Component.Line = api.engine.getComponent(lineEntity, api.type.ComponentType.LINE)
				if lineComp then
					local vehiclesOnLine = api.engine.system.transportVehicleSystem.getLineVehicles(lineEntity)
					if add then
						vmApi.addVehiclesToVehicleListAndSelect(vehiclesOnLine)
					else
						vmApi.removeVehiclesFromVehicleList(vehiclesOnLine)
					end
				end
			end

			local addOrRemoveVehiclesForDepot = function(depotEntity : Engine.Entity, add : boolean)
				local vehiclesInDepot = api.engine.system.transportVehicleSystem.getDepotVehicles(depotEntity)
				if add then
					vmApi.addVehiclesToVehicleListAndSelect(vehiclesInDepot)
				else
					vmApi.removeVehiclesFromVehicleList(vehiclesInDepot)
				end
			end

			if vmApi then
				for __, lineEAR in ipairs(params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected) do
					addOrRemoveVehiclesForLine(lineEAR.entity, addToSelection)
				end
				-- for __, lineEAR in ipairs(params.lineManagerStateRef:get().lineListEntitiesUnselected) do
					-- addOrRemoveVehiclesForLine(lineEAR.entity, addToSelection)
				-- end

				for __, depotEAR in ipairs(params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected) do
					addOrRemoveVehiclesForDepot(depotEAR.entity, addToSelection)
				end
				-- for __, depotEAR in ipairs(params.lineManagerStateRef:get().depotListEntitiesUnselected) do
					-- addOrRemoveVehiclesForDepot(depotEAR.entity, addToSelection)
				-- end
			end
		end,
		value = checkboxValue,
		triStateSupport = false, -- do not allow indeterminate state to be set externally
	})

	table.insert(actionListChildren, makeCheckboxWithProblemCount(
		allLinesCheckbox,
		params.commonParams.countDataState:old().countLinesWithProblems
	))

	local selectedLineColor : Vec4f = color_util.toVec4(defaultColorsRef:get().Invisible)
	if #params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected == 1 then
		local lineEAR = params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected[1]
		if not entity_util.entityChanged0(lineEAR) then
			local colorComponent = api.engine.getComponent(lineEAR.entity, api.type.ComponentType.COLOR) as Engine.Component.Color
			if colorComponent then
				selectedLineColor = api.type.Vec4f.new(colorComponent.color, 1)
			end
		end
	end

	table.insert(actionListChildren, builtin.Component{
		meta = { class = "horizontal-spacer" }
	})

	local function transformFilterState(transformFn : function(cur : StatisticsReactUtil.FilterState) : StatisticsReactUtil.FilterState)
		params.filterState:transform(transformFn)
	end

	if params.filterConfigState:old() then
		table.insert(actionListChildren, builtin.Component{
			meta = {
				disableFocusableRecursive = moreOptionsLocalState:old(),
			},
			layout = builtin.BoxLayout{
				child = statistics_react_util.createCarrierCategoryFilter(
					params.filterConfigState:old().categories,
					"Lines",
					function() : StatisticsReactUtil.FilterState
						return params.filterState:old()
					end,
					transformFilterState
				),
			}
		})
	end

	table.insert(actionListChildren, builtin.Component{
		meta = { class = "horizontal-spacer" }
	})

	table.insert(actionListChildren, builtin.Component{
		layout = builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					h = -1,
					v = -1,
					item = builtin.Button(react.ref(params.commonParams.btnNewLineRef), {
						meta = {
							disableFocusableRecursive = moreOptionsLocalState:old(),
							class = "primary, fixed-size-button, btn-new-line",
							enabled = params.commonParams.btnNewLineEnabled,
							tag = "menu.management.newLineButton",
							onFocusChange = function(focus : boolean)
								focusedLastState:set(focus)
							end,
						},
						content = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Horizontal,
							children = {
								builtin.ImageView{
									path = params.commonParams.iconPaths.abNewLine,
									scaling = builtin.type.ImageViewScaling.AutoFit,
								},
								builtin.TextView{
									meta = {
										class = "font-scale-body",
									},
									text = _("New Line")
								},
							},
						},
						onClick = function()
							params.commonParams.btnNewLineOnClick()
						end
					}),
				},
			},
		},
	})

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Horizontal,
		children = actionListChildren,
	}
end)

local record ToggleSelectLineOrDepotParams
	vehicleManagerRef : ReactRefWrapApiT<VehicleManagerParams, VehicleManagerApi>
	lineAndDepotListApi : LineAndDepotListApi
	areAnyVehiclesInDepot : function(vehicleDepotEntity : Engine.Entity) : boolean
	entity : Engine.Entity
	problemCacheState : ReactStateT<ProblemCache>
end

local ToggleSelectLine = react.RegisterRecipe("ToggleSelectLine", function(params : ToggleSelectLineOrDepotParams) : TreeNodeId
	local vmApi : VehicleManagerApi
	if params.vehicleManagerRef:get() then
		vmApi = params.vehicleManagerRef:get():getApi()
	end

	local isSelectedState = engine_react_util.useStepState(function() : boolean
		return params.lineAndDepotListApi.isLineSelected(params.entity)
	end)

	local vehicleProblemsCountState : ReactStateT<integer> = react.useState(0)
	react.onStepTimer(function()
		local problemCount = 0
		if params.problemCacheState:old() then
			problemCount = params.problemCacheState:old().line2vehicleProblemCount[params.entity] or 0
		end

		if not table_util.deepEquals (vehicleProblemsCountState:old(), problemCount) then
			vehicleProblemsCountState:set(problemCount)
		end
	end)

	local lineCheckbox = builtin.CheckBox{
		meta = {
			tooltip = makeVehicleProblemsTooltip(vehicleProblemsCountState:old()),
		},
		onValueChange = function(__val : integer)
			params.lineAndDepotListApi.selectLine(
				params.entity,
				not isSelectedState:old(),
				nil
			)
			if vmApi then
				local vehiclesOnLine = api.engine.system.transportVehicleSystem.getLineVehicles(params.entity)
				if not isSelectedState:old() then
					vmApi.addVehiclesToVehicleListAndSelect(vehiclesOnLine)
				else
					vmApi.removeVehiclesFromVehicleList(vehiclesOnLine)
				end
			end
		end,
		value = isSelectedState:old() and 1 or 0,
	}

	return builtin.FloatingLayout{
		children = {
			builtin.FloatingLayoutChild{
				h = -1,
				v = -1,
				item = lineCheckbox,
			},
			vehicleProblemsCountState:old() > 0 and builtin.FloatingLayoutChild{
				h = 1.0,
				v = 0.0,
				overflowMode = builtin.type.FloatingLayoutOverflowMode.Overflow,
				item = builtin.TextView{
					meta = {
						class = "overflow-mode, font-scale-annotation, line-problem-count",
						mouseTransparent = true,
					},
					text = lang_util.formatInt(vehicleProblemsCountState:old()),
				}
			} or nil,
		},
	}
end)

local ToggleSelectDepot = react.RegisterRecipe("ToggleSelectDepot", function(params : ToggleSelectLineOrDepotParams) : TreeNodeId
	local vmApi : VehicleManagerApi
	if params.vehicleManagerRef:get() then
		vmApi = params.vehicleManagerRef:get():getApi()
	end

	local isSelectedState = engine_react_util.useStepState(function() : boolean
		return params.lineAndDepotListApi.isDepotSelected(params.entity)
	end)

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Horizontal,
		children = {
			builtin.CheckBox{
				onValueChange = function(__val : integer)
					params.lineAndDepotListApi.selectDepot(
						params.entity,
						not isSelectedState:old()
					)
					if vmApi then
						local vehiclesInDepot = api.engine.system.transportVehicleSystem.getDepotVehicles(params.entity)
						if not isSelectedState:old() then
							vmApi.addVehiclesToVehicleListAndSelect(vehiclesInDepot)
						else
							vmApi.removeVehiclesFromVehicleList(vehiclesInDepot)
						end
					end
				end,
				value = isSelectedState:old() and 1 or 0,
			},
		},
	}
end)

local record LineAndDepotListEntryParams
	commonParams : CommonActionParams
	lineAndDepotListApi : LineAndDepotListApi
end

local LineAndDepotListEntry = react.RegisterRecipe("LineAndDepotListEntry", function(params : Builtin.TableCellParam) : TreeNodeId
	local entity = params.rowKey
	local userParams = params.userParam as LineAndDepotListEntryParams

	if not api.engine.entityExists(entity) then
		return nil
	end

	local line : Engine.Component.Line = api.engine.getComponent(entity, api.type.ComponentType.LINE)
	local depot : Engine.Component.VehicleDepot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT)

	if (line == nil) and (depot == nil or (#depot.inNodes + #depot.outNodes == 0)) then
		return nil
	end

	local gamepadFocusedState = react.useState(false) as ReactStateT<boolean>
	local hoveredState = react.useState(false) as ReactStateT<boolean>
	local lockedState = react.useState(false)
	local editModeState : ReactStateT<boolean> = react.useState(false)

	react.onMouseEvent(function (evt :  Gui.Mouse.Event) : boolean
		if evt.type == api.gui.mouse.Event.Type.Entered then
			hoveredState:set(true)
		end
		if evt.type == api.gui.mouse.Event.Type.Exited then
			hoveredState:set(false)
		end
		if evt.type == api.gui.mouse.Event.Type.Pressed then
			if editModeState:old() then
				lockedState:set(true)
			end
		elseif evt.type == api.gui.mouse.Event.Type.Released then
			lockedState:set(false)
		end
		return false
	end)

	local isLine = line ~= nil

	local lineHasProblemsState : ReactStateT<boolean> = react.useState(false)


	local scrollToRenameState : ReactStateT<boolean> = react.useState(false)
	local oldLineNameState : ReactStateT<string> = react.useState("")
	react.onStep(function()
		local currentName = api.engine.util.getEntityName(entity)
		if oldLineNameState:old() ~= currentName then
			if scrollToRenameState:old() then
				userParams.lineAndDepotListApi.scrollToRow(entity, true)
				scrollToRenameState:set(false)
			end
			oldLineNameState:set(currentName)
		end
	end)

	local firstRef = react.useNodeRef()
	local lastRef = react.useNodeRef()
	local focusedFirstState : ReactStateT<boolean> = react.useState(false)
	local focusedLastState : ReactStateT<boolean> = react.useState(false)
	if focusedFirstState:old() then
		react.useInputAction("IA_NAV_LEFT", react.iaHandler(function()
			if lastRef:get() then
				lastRef:get():focus()
			end
		end))
	end
	if focusedLastState:old() then
		react.useInputAction("IA_NAV_RIGHT", react.iaHandler(function()
			if firstRef:get() then
				firstRef:get():focus()
			end
		end))
	end

	local makeNameState = function() : string
		local nameComp = api.engine.util.getEntityName(entity)
		if nameComp then
			return nameComp
		end
		return ""
	end
	local makeNameCommand = function(newName : string) : Command<EntitySetNameCommandData>, nil
		return api.cmd.makeEntitySetNameCmd(entity, newName)
	end
	local nameState, commitName : ReactStateT<string>, function(string) = engine_react_util.useStepStateTimerWithCommit(makeNameState, 0, makeNameCommand)

	local isSelectedState = engine_react_util.useStepState(function() : boolean
		if not isLine then
			return userParams.lineAndDepotListApi.isDepotSelected(entity)
		end
		return userParams.lineAndDepotListApi.isLineSelected(entity)
	end)

	local makeNameButton = function(onClickOverride : function(), tooltip : string) : TreeNodeId
		return builtin.Component{
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					builtin.Component{
						layout = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Horizontal,
							children = {
								editModeState:old() and gui_react_util.FocusTextInputField{
									class = "font-scale-body",
									value = nameState:old(),
									onValueChange = function(newName : string)
									local currentName = api.engine.util.getEntityName(entity)
									if newName ~= currentName then
											commitName(newName)
											scrollToRenameState:set(true)
										end
										editModeState:set(false)
									end,
									onCancel = function()
										editModeState:set(false)
									end
								} or builtin.Component{
									meta = {
										forceFocusable = not userParams.commonParams.gamepadInputModeState:old(),
									},
									layout = builtin.BoxLayout{
										orientation = builtin.type.Orientation.Horizontal,
										children = {
											builtin.Button{
												meta = {
													class = "name",
													tooltip = tooltip,
												},
												content = builtin.TextView{
													meta = {
														class = "font-scale-body, "
															.. (lineHasProblemsState:old() and "problem, " or ""),
													},
													text = nameState:old(),
												},
												onClick = onClickOverride,
												onDoubleClick = function()
													api.gui.camera.followEntity(entity, true)
												end,
											},
											builtin.Button{
												meta = {
													enabled = not userParams.commonParams.gamepadInputModeState:old(),
												},
												content = builtin.Component{
													meta = {
														class = "text-gap",
														tooltip = tooltip,
													},
												},
												onClick = onClickOverride,
												onDoubleClick = function()
													api.gui.camera.followEntity(entity, true)
												end,
											},
										},
									},
								}
							},
						},
					},
				},
			},
		}
	end

	local onClickOverride : function()
	local tooltipSelectLine = _("Select Line")
	local tooltipSelectDepot = _("Select Depot")
	local children : {TreeNodeId} = {}

	local showButtons = gamepadFocusedState:old() or hoveredState:old()

	if isLine then
		react.onStep(function()
			local lineHasProblems = false
			if userParams.commonParams.problemCacheState:old() then
				lineHasProblems = userParams.commonParams.problemCacheState:old().line2problem[entity] or false
			end

			if not table_util.deepEquals(lineHasProblemsState:old(), lineHasProblems) then
				lineHasProblemsState:set(lineHasProblems)
			end
		end)

		children[#children + 1] = ToggleSelectLine(react.ref(firstRef), {
			meta = {
				onFocusChange = function(focus : boolean)
					focusedFirstState:set(focus)
				end,
			},
			lineAndDepotListApi = userParams.lineAndDepotListApi,
			vehicleManagerRef = userParams.commonParams.vehicleManagerRef,
			entity = entity,
			problemCacheState = userParams.commonParams.problemCacheState,
		})

		children[#children + 1] = line_react_util.ColorWidget{
			tooltip = _("Open Line Window"),
			entity = entity,
			onClick = function()
				local wc = userParams.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				userParams.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)

				react.fireEvent(nil, "selectEntity", {entity = entity, stack = true})
			end,
		}

		onClickOverride = function()
			local vmApi : VehicleManagerApi = nil
			if userParams.commonParams.vehicleManagerRef:get() then
				vmApi = userParams.commonParams.vehicleManagerRef:get():getApi()
			end

			if vmApi and userParams.lineAndDepotListApi then
				selectSingleLineAndOverrideSelectionOnClick(
					vmApi,
					userParams.lineAndDepotListApi,
					userParams.commonParams.creatingNewLineState,
					entity,
					false,
					nil
				)
			end
		end

		children[#children + 1] = makeNameButton(onClickOverride, tooltipSelectLine)

		children[#children + 1] = line_react_util.ManagerNotificationWidget(entity)

		children[#children + 1] = builtin.Button{
			meta = {
				class = "statistics-button" .. (showButtons and "" or ", hidden-button"),
				tooltip = _("Rename Line"),
			},
			content = builtin.ImageView{
				path = userParams.commonParams.iconPaths.rowRename,
				scaling = builtin.type.ImageViewScaling.AutoFit,
			},
			onClick = function()
				if not lockedState:old() then
					editModeState:set(not editModeState:old())
				end
			end,
		}

		children[#children + 1] = line_react_util.LocateButton{
			meta = {
				class = "line-depot" .. (showButtons and "" or ", hidden-button"),
			},
			entity = entity,
			tooltip = _("Locate Line"),
			iconPathOverride = userParams.commonParams.iconPaths.rowLocate,
		}

		local makeDeleteState = function() : integer
			if userParams.commonParams.gameCtx.filters:get().protectedEntities[entity] then
				return -1
			end
			local vehicles = api.engine.system.transportVehicleSystem.getLineVehicles(entity)
			for __, vehicleEntityId in ipairs(vehicles) do
				if userParams.commonParams.gameCtx.filters:get().protectedEntities[vehicleEntityId] then
					return -1
				end
			end
			return #vehicles
		end
		local makeDeleteCommands = function(numberOfVehicles : integer) : {{Command<ICommandData>, function(ICommandData, boolean, {{Engine.Entity, Engine.Revision}})}}
			local vehicles = api.engine.system.transportVehicleSystem.getLineVehicles(entity)
			if #vehicles ~= numberOfVehicles then
				log.warning("Number of vehicles on line changed, no command has been created...")
				return nil
			end

			local missionError = api.gui.fireGuiScriptEvent("management.line", "delete", entity) as boolean
			if missionError then
				return nil
			end

			local cmds : {{Command<ICommandData>, function(ICommandData, boolean, {{Engine.Entity, Engine.Revision}})}} = {}
			if #vehicles > 0 then
				for __, vehicleEntityId in ipairs(vehicles) do
					if userParams.commonParams.gameCtx.filters:get().protectedEntities[vehicleEntityId] then
						return nil
					end
				end
				local sellCmd = api.cmd.makeVehicleSellCmd(vehicles)
				cmds[#cmds + 1] = {sellCmd as Command<ICommandData>, nil}
			end

			local destroyCmd = api.cmd.makeLineDestroyCmd(entity)
			cmds[#cmds + 1] = {destroyCmd as Command<ICommandData>, nil}

			return cmds
		end
		-- Note: conditional state has "replacement" state in else to prevent state index shifting [1]
		local deleteState, commitDelete : ReactStateT<integer>, function(integer) = engine_react_util.useStepStateMulti(makeDeleteState, makeDeleteCommands, nil, nil, true)
		local wrappedCommitDelete = function()
			if api.engine.entityExists(entity) then
				local wc = userParams.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				userParams.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)
				userParams.lineAndDepotListApi.removeLineFromList(entity, true)
				userParams.commonParams.switchMode{"MAIN", nil}
				commitDelete(makeDeleteState())
			end
		end

		children[#children + 1] = builtin.Button(react.ref(lastRef), {
			meta = {
				class = "statistics-button"
					.. (deleteState:old() == 0 and ", delete-line-sfx" or "")
					.. (showButtons and "" or ", hidden-button"),
				tooltip = _("Delete Line"),
				tag = "menu.management.deleteLineButton",
				onFocusChange = function(focus : boolean)
					focusedLastState:set(focus)
				end,
			},
			content = builtin.ImageView{
				path = userParams.commonParams.iconPaths.rowDelete,
				scaling = builtin.type.ImageViewScaling.AutoFit,
			},
			onClick = function()
				if deleteState:old() == -1 then
					userParams.commonParams.addFeedback(_("Cannot delete line."), "Error", nil, 1)
				elseif deleteState:old() > 0 then
					userParams.commonParams.addFeedback(_("Deleting the line will sell all vehicles on the line."), "Question", {
						onAccept = function()
							wrappedCommitDelete()
						end,
						acceptText = _("Delete"),
						soundOnAccept = "DeleteLine",
					}, 2)
				else
					wrappedCommitDelete()
				end
			end,
		})
	else
		children[#children + 1] = ToggleSelectDepot(react.ref(firstRef), {
			meta = {
				onFocusChange = function(focus : boolean)
					focusedFirstState:set(focus)
				end,
			},
			lineAndDepotListApi = userParams.lineAndDepotListApi,
			vehicleManagerRef = userParams.commonParams.vehicleManagerRef,
			areAnyVehiclesInDepot = userParams.commonParams.areAnyVehiclesInDepot,
			entity = entity,
		})

		children[#children + 1] = builtin.Button{
			meta = {
				class = "statistics-button",
				tooltip = _("Open Depot Window"),
			},
			content = builtin.ImageView{
				path = line_react_util.getSlimDepotIconPath(entity),
				scaling = builtin.type.ImageViewScaling.AutoFit,
			},
			onClick = function()
				local wc = userParams.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				userParams.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)

				react.fireEvent(nil, "selectEntity", {entity = entity, stack = true})
			end,
		}

		onClickOverride = function()
			local vmApi : VehicleManagerApi = nil
			if userParams.commonParams.vehicleManagerRef:get() then
				vmApi = userParams.commonParams.vehicleManagerRef:get():getApi()
			end
			if vmApi then
				vmApi.clearVehicleList()
				userParams.lineAndDepotListApi.deselectAllLinesAndDepots()
				userParams.lineAndDepotListApi.selectDepot(
					entity,
					not userParams.lineAndDepotListApi.isDepotSelected(entity)
				)
				local vehiclesInDepot = api.engine.system.transportVehicleSystem.getDepotVehicles(entity)
				vmApi.addVehiclesToVehicleListAndSelect(vehiclesInDepot)
			end
		end

		children[#children + 1] = makeNameButton(onClickOverride, tooltipSelectDepot)

		children[#children + 1] = line_react_util.ManagerNotificationWidget(entity)

		children[#children + 1] = builtin.Button{
			meta = {
				class = "statistics-button" .. (showButtons and "" or ", hidden-button"),
				tooltip = _("Rename Depot"),
			},
			content = builtin.ImageView{
				path = userParams.commonParams.iconPaths.rowRename,
				scaling = builtin.type.ImageViewScaling.AutoFit,
			},
			onClick = function()
				if not lockedState:old() then
					editModeState:set(not editModeState:old())
				end
			end,
		}

		children[#children + 1] = line_react_util.LocateButton{
			meta = {
				class = "line-depot" .. (showButtons and "" or ", hidden-button"),
			},
			entity = entity,
			tooltip = _("Locate Depot"),
			iconPathOverride = userParams.commonParams.iconPaths.rowLocate,
		}

		children[#children + 1] = builtin.Button(react.ref(lastRef), {
			meta = {
				class = "statistics-button" .. (showButtons and "" or ", hidden-button"),
				tooltip = _("Buy Vehicles In Depot"),
				onFocusChange = function(focus : boolean)
					focusedLastState:set(focus)
				end,
			},
			content = builtin.ImageView{
				path = userParams.commonParams.iconPaths.rowCart,
				scaling = builtin.type.ImageViewScaling.AutoFit,
			},
			onClick = function()
				openDepotWindow(
					userParams.commonParams.gameCtx,
					entity,
					nil,
					userParams.commonParams.vehicleStoreOpenForDepotAndLineRef,
					userParams.commonParams.openVehicleDepot
				)
				local selectedLineCount = #userParams.commonParams.lineManagerStateRef:get().lineListEntitiesSelected
				if selectedLineCount == 0 then
					local vmApi : VehicleManagerApi = nil
					if userParams.commonParams.vehicleManagerRef:get() then
						vmApi = userParams.commonParams.vehicleManagerRef:get():getApi()
					end

					if vmApi and userParams.lineAndDepotListApi then
						selectSingleDepotAndOverrideSelectionOnClick(
							vmApi,
							userParams.lineAndDepotListApi,
							userParams.commonParams.creatingNewLineState,
							entity,
							false
						)
					end
				end
			end,
		})
	end

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return userParams.commonParams.moreOptionsState:old()
	end)

	return builtin.BoxLayout{
		children = {
			(userParams.commonParams.gamepadInputModeState:old() and moreOptionsLocalState:old())
			and builtin.Component{
				meta = {
					forceFocusable = true,
					onFocusChange = function(focused : boolean)
						userParams.commonParams.deleteSellRef:set({
							focusedLine = focused and entity or nil,
							focusedVehicle = userParams.commonParams.deleteSellRef:get().focusedVehicle,

							focusedStopLine = userParams.commonParams.deleteSellRef:get().focusedStopLine,
							focusedStopNumber = userParams.commonParams.deleteSellRef:get().focusedStopNumber,
							focusedApiStopIndex0 = userParams.commonParams.deleteSellRef:get().focusedApiStopIndex0,
						})
					end,
				},
			} or nil,
			builtin.Button{
				meta = {
					disableFocusableRecursive = moreOptionsLocalState:old(),
					class = "row, "
						.. (isSelectedState:old() and "selected," or "")
						.. ((not isLine and not userParams.commonParams.areAnyVehiclesInDepot(entity)) and "empty-depot," or ""),
					tooltip = isLine and tooltipSelectLine or tooltipSelectDepot,
					onFocusChange = function(focus : boolean)
						if userParams.commonParams.gamepadInputModeState:old() then
							gamepadFocusedState:set(focus)
						end
					end,
				},
				content = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = children,
				},
				onClick = onClickOverride,
				onDoubleClick = function()
					api.gui.camera.followEntity(entity, true)
				end,
			},
		},
	}
end)

local LineAndDepotList = react.RegisterRecipe("LineAndDepotList", function(params : LineAndDepotListParams) : TreeNodeId
	local record TableState
		rowKeys : {Engine.Entity}
	end
	local tableState : ReactStateT<TableState> = engine_react_util.useStepState(function() : TableState
		local rowKeys : {Engine.Entity} = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())

		local depotKeys : {Engine.Entity} = api.engine.getEntitiesWithComponent(api.type.ComponentType.VEHICLE_DEPOT, {
			requireOwnedByPlayer = api.engine.util.getPlayer(),
			filterFn = function(entity : Engine.Entity) : boolean
				local vehicleDepot : Engine.Component.VehicleDepot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT)
				return vehicleDepot and (#vehicleDepot.inNodes > 0 or #vehicleDepot.outNodes > 0)
			end,
		})
		for __, depotKey in ipairs(depotKeys) do
			rowKeys[#rowKeys + 1] = depotKey
		end

		return {
			rowKeys = rowKeys,
		}
	end)

	local tableRef = react.useNodeRef(builtin.DataTable)
	react.setPreferredFocusChild(tableRef)

	local filterCategories : {StatisticsRecipeParams.Category} = {
		{
			imageFile = params.commonParams.iconPaths.abFilterRoad,
			tooltip = _("Show Road Lines"),
			carrier = api.type["enum"].Carrier.ROAD,
		},
		{
			imageFile = params.commonParams.iconPaths.abFilterTram,
			tooltip = _("Show Tram Lines"),
			carrier = api.type["enum"].Carrier.TRAM,
		},
		{
			imageFile = params.commonParams.iconPaths.abFilterRail,
			tooltip = _("Show Train Lines"),
			carrier = api.type["enum"].Carrier.RAIL,
		},
		{
			imageFile = params.commonParams.iconPaths.abFilterWater,
			tooltip = _("Show Ship Lines"),
			carrier = api.type["enum"].Carrier.WATER,
		},
		{
			imageFile = params.commonParams.iconPaths.abFilterAir,
			tooltip = _("Show Air Lines"),
			carrier = api.type["enum"].Carrier.AIR,
		}
	}

	local declareFilterOnMount = function(categories : {StatisticsRecipeParams.Category}, onlyVisible : StatisticsRecipeParams.OnlyVisible)
		return params.filterConfigState:set({categories = categories, onlyVisible = onlyVisible})
	end

	react.onMount(function()
		declareFilterOnMount(filterCategories, nil)
	end)

	local searchString = params.filterState:old().search or ""
	local filterShowCategories = params.filterState:old().category
	local filterShowOnlyVisible = params.filterState:old().onlyVisible

	local fnUserFilter = function(key : integer) : boolean
		if not api.engine.entityExists(key) then
			return false
		end

		local line : Engine.Component.Line = api.engine.getComponent(key, api.type.ComponentType.LINE)
		if line then
			if filterShowOnlyVisible and not api.gui.byEntity.isLineEmptyOrVisible(key) then
				return false
			end

			local allowedCarriers = statistics_react_util.getCarrierFilterFromCategories(filterCategories, filterShowCategories)
			if allowedCarriers ~= nil then
				if not line_util.filterLine(allowedCarriers, key) then
					return false
				end
			end

			if searchString == "" then
				return true
			end

			local lineName = api.engine.util.getEntityName(key)
			if lang_util.stringContains(lineName, searchString) then
				return true
			end

			local cargoInfo : {CargoUtil.VehicleCargo} = cargo_util.calculateSortedLineCargoInfo(key)
			for __, info in ipairs(cargoInfo) do
				local cargoName = cargo_util.getCargoNameById(info.cargoType)
				if lang_util.stringContains(cargoName, searchString) then
					return true
				end
			end
		end

		local depot : Engine.Component.VehicleDepot = api.engine.getComponent(key, api.type.ComponentType.VEHICLE_DEPOT)
		if depot and (#depot.inNodes > 0 or #depot.outNodes > 0) then
			if filterShowOnlyVisible and not api.gui.byEntity.isVisible(key) then
				return false
			end

			local allowedCarriers  = statistics_react_util.getCarrierFilterFromCategories(filterCategories, filterShowCategories)
			if allowedCarriers ~= nil then
				if not statistics_react_util.areCarriersContainedInTransportModes(depot.transportModes, allowedCarriers) then
					return false
				end
			end

			if searchString == "" then
				return true
			end

			local depotName = api.engine.util.getEntityName(key)
			if lang_util.stringContains(depotName, searchString) then
				return true
			end
		end

		return false
	end

	local lineAndDepotListApi : LineAndDepotListApi = {
		scrollToRow = function(entity : Engine.Entity, forceTopAlign : boolean)
			local tableApi = tableRef:get() and tableRef:get():getApi()
			if tableApi then
				tableApi.scrollToRowKey(entity as integer, forceTopAlign)
			end
		end,
		isLineSelected = function(lineEntity : Engine.Entity) : boolean
			for __, selectedEAR in ipairs(params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected) do
				if lineEntity == selectedEAR.entity then
					return true
				end
			end
			return false
		end,
		isDepotSelected = function(depotEntity : Engine.Entity) : boolean
			for __, selectedEAR in ipairs(params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected) do
				if depotEntity == selectedEAR.entity then
					return true
				end
			end
			return false
		end,
		selectLine = function(lineEntity : Engine.Entity, selectEntity : boolean, selectSegment : integer)
			local selectedLines : {EntityUtil.EntityAndRevision} = {}
			local selectedLinesOld = params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected
			local unselectedLinesOld = params.commonParams.lineManagerStateRef:get().lineListEntitiesUnselected

			if selectEntity then
				for __, unselectedLineEAR in ipairs(unselectedLinesOld) do
					if (lineEntity == unselectedLineEAR.entity) then
						table.insert(selectedLines, unselectedLineEAR)
					end
				end

				for __, selectedLineEAR in ipairs(selectedLinesOld) do
					table.insert(selectedLines, selectedLineEAR)
				end
			else
				for __, selectedLineEAR in ipairs(selectedLinesOld) do
					if (lineEntity ~= selectedLineEAR.entity) then
						table.insert(selectedLines, selectedLineEAR)
					end
				end
			end

			params.commonParams.lineManagerStateRef:set(makeLineManagerState(
				selectedLines,
				params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected,
				nil
			))

			local wc = params.commonParams.gameCtx.windowContainer:get():getApi()
			wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)

			if #selectedLines == 1 then
				local reactLine = line_util.getReactLineFromGameState(selectedLines[1].entity)
				local insertAt = selectSegment or #reactLine.path
				params.commonParams.switchMode({"SEGMENT", insertAt})
			else
				params.commonParams.switchMode({"MAIN", nil})
			end
		end,
		selectDepot = function(depotEntity : Engine.Entity, selectEntity : boolean)
			local selectedDepots : {EntityUtil.EntityAndRevision} = {}
			local selectedDepotsOld = params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected
			local unselectedDepotsOld = params.commonParams.lineManagerStateRef:get().depotListEntitiesUnselected

			if selectEntity then
				for __, unselectedDepotEAR in ipairs(unselectedDepotsOld) do
					if (depotEntity == unselectedDepotEAR.entity) then
						table.insert(selectedDepots, unselectedDepotEAR)
					end
				end

				for __, selectedDepotEAR in ipairs(selectedDepotsOld) do
					table.insert(selectedDepots, selectedDepotEAR)
				end

				params.commonParams.addDepotVehiclesToDepotVehicleCompare(depotEntity)
			else
				for __, selectedDepotEAR in ipairs(selectedDepotsOld) do
					if (depotEntity ~= selectedDepotEAR.entity) then
						table.insert(selectedDepots, selectedDepotEAR)
					end
				end

				params.commonParams.removeDepotVehiclesFromDepotVehicleCompare(depotEntity)
			end

			params.commonParams.lineManagerStateRef:set(makeLineManagerState(
				params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected,
				selectedDepots,
				nil
			))
		end,
		deselectAllLinesAndDepots = function()
			params.commonParams.lineManagerStateRef:set(makeLineManagerState({}, {}, nil))
			params.commonParams.clearDepotVehicleCompare()
		end,
		removeLineFromList = function(lineEntity : Engine.Entity, deleteLine : boolean)
			local selectedLines : {EntityUtil.EntityAndRevision} = {}
			local selectedLinesOld = params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected

			for __, selectedLineEAR in ipairs(selectedLinesOld) do
				if lineEntity ~= selectedLineEAR.entity then
					table.insert(selectedLines, selectedLineEAR)
				end
			end

			params.commonParams.lineManagerStateRef:set(makeLineManagerState(
				selectedLines,
				params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected,
				deleteLine and lineEntity or nil
			))

			params.commonParams.switchMode({"MAIN", nil})
		end,
	}
	react.provideApi(lineAndDepotListApi)

	react.onStep(function()
		local scrollToLine = params.scrollToNewLineRef:get()
		if scrollToLine then
			if table_util.arrayContains(tableState:old().rowKeys, scrollToLine) then
				lineAndDepotListApi.scrollToRow(scrollToLine, true)
				params.scrollToNewLineRef:set(nil)
			end
		end
	end)

	local columns = {
		builtin.ColumnDesc{
			name = _("Name"),
			recipe = LineAndDepotListEntry,
			getCompareValue = function(rowKey : integer) : { boolean, string }
				local line : Engine.Component.Line = api.engine.getComponent(rowKey, api.type.ComponentType.LINE)
				local isDepot : boolean = (line == nil) and true or false

				local name = api.engine.util.getEntityName(rowKey)

				return { isDepot, name }
			end,
			weight = 1.0,
		}
	}

	local userParams : LineAndDepotListEntryParams = {
		commonParams = params.commonParams,
		lineAndDepotListApi = lineAndDepotListApi,
	}

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.DataTable(react.ref(tableRef), {
				disableSortKey = true,
				columns = columns,
				rowKeys = tableState:old().rowKeys,
				userParam = userParams,
				scrollPolicyVertical = builtin.type.ScrollBarPolicy.AsNeededButAlwaysReserveSpace,
				scrollPolicyHorizontal = builtin.type.ScrollBarPolicy.AlwaysOff,
				fnUserFilter = fnUserFilter,
				preferredInitialSelectionRowKey = params.preferredInitialSelectionRowKey:old(),
				keyboardNavigation = true,
			}),
		},
	}
end) as RecipeWithApi<LineAndDepotListParams, LineAndDepotListApi>

local record InfoBoxParams
	infoTitle : string
	infoMessage : string
	infoCancelMessage : string
	commonParams : CommonActionParams
end

local InfoBox = react.RegisterRecipe("InfoBox", function(params : InfoBoxParams) : TreeNodeId
	local children = {
		builtin.Component{
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					builtin.Component{
						meta = { class = "info-box" },
						layout = builtin.BoxLayout{
							children = {
								builtin.Component{
									meta = { class = "horizontal-spacer" },
								},
								builtin.TextView{
									meta = { class = "font-scale-title-4" },
									text = params.infoTitle,
								},
								builtin.Component{
									meta = { class = "horizontal-spacer" },
								},
							},
						},
					},
				},
			},
		},
		builtin.Component{
			layout = builtin.FloatingLayout{
				children = {
					builtin.FloatingLayoutChild{
						h = -1,
						v = -1,
						item = builtin.TextView{
							meta = {
								class = "empty-text, font-scale-body"
							},
							text = params.infoMessage,
						},
					},
					(not params.commonParams.gamepadInputModeState:old() and params.infoCancelMessage ~= nil) and builtin.FloatingLayoutChild{
						h = 1,
						v = 1,
						item = builtin.Button{
							meta = {
								class = "primary",
								tooltip = params.infoCancelMessage,
							},
							content = builtin.TextView{
								meta = { class = "font-scale-body" },
								text = _("Cancel"),
							},
							onClick = function()
								params.commonParams.creatingNewLineState:set(false)
								params.commonParams.switchMode({"MAIN", nil})
							end,
						},
					} or nil,
				}
			}
		},
	}

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = children,
	}
end)

local record LineManagerParams
	lineManagerActionButtonsRef : ReactRefWrap
	scrollToNewLineRef : ReactRefT<Engine.Entity>

	filterState : ReactStateT<StatisticsReactUtil.FilterState>
	filterConfigState : ReactStateT<LVMFilterConfig>
	preferredInitialSelectionRowKey : ReactStateT<Engine.Entity>

	commonParams : CommonActionParams
end

local LineManager = react.RegisterRecipe("LineManager", function(params : LineManagerParams) : TreeNodeId
	local lineManagerActionButtons = LineManagerActionButtons(react.ref(params.lineManagerActionButtonsRef), {
		meta = {
			onFocusChange = function(focused)
				params.commonParams.lvmActionButtonsState:set(focused and "LMAB" or "NONE")
			end,
		},

		filterState = params.filterState,
		filterConfigState = params.filterConfigState,

		commonParams = params.commonParams,

		scrollToRow = function(entity : Engine.Entity)
			params.commonParams.lineAndDepotListRef:get():getApi().scrollToRow(entity, true)
		end,
	})

	local hasPlayerLinesOrDepotsState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		local playerLines : {Engine.Entity} = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())
		local playerDepots : {Engine.Entity} = api.engine.getEntitiesWithComponent(api.type.ComponentType.VEHICLE_DEPOT, {
			requireOwnedByPlayer = api.engine.util.getPlayer(),
			filterFn = function(entity : Engine.Entity) : boolean
				local vehicleDepot : Engine.Component.VehicleDepot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT)
				return vehicleDepot and (#vehicleDepot.inNodes > 0 or #vehicleDepot.outNodes > 0)
			end,
		})
		return (#playerLines > 0) or (#playerDepots > 0)
	end)

	local noLinesHelper = (not hasPlayerLinesOrDepotsState:old() and not params.commonParams.creatingNewLineState:old()) and builtin.Component{
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				gui_react_util.makeHorizontalSpacer(),
				builtin.TextView{
					meta = {
						class = "empty-text, font-scale-body"
					},
					text = _("No lines or depots exist."),
				},
				gui_react_util.makeHorizontalSpacer(),
			},
		},
	} or nil

	local lineAndDepotList = LineAndDepotList(react.ref(params.commonParams.lineAndDepotListRef), {
		filterState = params.filterState,
		filterConfigState = params.filterConfigState,

		lineManagerActionButtonsRef = params.lineManagerActionButtonsRef,
		preferredInitialSelectionRowKey = params.preferredInitialSelectionRowKey,
		scrollToNewLineRef = params.scrollToNewLineRef,

		commonParams = params.commonParams,
	})

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			lineManagerActionButtons,
			noLinesHelper,
			lineAndDepotList,
		},
	}
end)

local record DepotIconDisplayParams
	depotEntity : Engine.Entity
end

local DepotIconDisplay = react.RegisterRecipe("DepotIconDisplay", function(params : DepotIconDisplayParams) : TreeNodeId
	local nameState : ReactStateT<string> = engine_react_util.useStepState(function() : string
		if params.depotEntity then
			return api.engine.util.getEntityName(params.depotEntity)
		end
		return _("Depot")
	end)

	local depotIcon = builtin.ImageView{
		meta = {
			class = "depot-icon",
			tooltip = nameState:old() or _("Depot"),
		},
		path = line_react_util.getSlimDepotIconPath(params.depotEntity),
		scaling = builtin.type.ImageViewScaling.AutoFit,
	}

	return builtin.BoxLayout{
		child = builtin.Component{
			meta = {
				class = "depot-bubble",
			},
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				child = depotIcon,
			},
		},
	}
end)

local record NameAndEntity
	name : string
	entity : Engine.Entity
end

local record LineOrDepotNameFillingParams
	commonParams : CommonActionParams
	entity : Engine.Entity
	isLine : boolean
	nameAndEntityState : ReactStateT<NameAndEntity>
end

local LineOrDepotNameFilling = react.RegisterRecipe("LineOrDepotNameFilling", function(params : LineOrDepotNameFillingParams) : TreeNodeId
	if not params.entity then
		return nil
	end

	local hoveredState : ReactStateT<boolean> = react.useState(false)
	local editModeState : ReactStateT<boolean> = react.useState(false)
	local lockedState : ReactStateT<boolean> = react.useState(false)

	react.onMouseEvent(function (evt :  Gui.Mouse.Event) : boolean
		if evt.type == api.gui.mouse.Event.Type.Entered then
			hoveredState:set(true)
		end
		if evt.type == api.gui.mouse.Event.Type.Exited then
			hoveredState:set(false)
		end
		if evt.type == api.gui.mouse.Event.Type.Pressed then
			if editModeState:old() then
				lockedState:set(true)
			end
		elseif evt.type == api.gui.mouse.Event.Type.Released then
			lockedState:set(false)
		end
		return false
	end)

	local makeNameState = function() : string
		local nameComp = api.engine.util.getEntityName(params.entity)
		if nameComp then
			return nameComp
		end
		return ""
	end
	local makeNameCommand = function(newName : string) : Command<EntitySetNameCommandData>, nil
		return api.cmd.makeEntitySetNameCmd(params.entity, newName)
	end
	local nameState, commitName : ReactStateT<string>, function(string) = engine_react_util.useStepStateTimerWithCommit(makeNameState, 0, makeNameCommand)

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Horizontal,
		children = {
			params.isLine and line_react_util.LineCargoDisplay{
				lineEntity = params.entity,
			} or DepotIconDisplay{
				depotEntity = params.entity,
			},
			builtin.Component{
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = {
						editModeState:old() and builtin.Component{
							meta = {
								class = "line-depot-name-textfield",
							},
							layout = builtin.BoxLayout{
								orientation = builtin.type.Orientation.Horizontal,
								children = {
									gui_react_util.FocusTextInputField{
										class = "font-scale-headline",
										value = nameState:old(),
										onValueChange = function(newName : string)
											local currentName = api.engine.util.getEntityName(params.entity)
											if newName ~= currentName then
												commitName(newName)
												-- scrollToRenameState:set(true)
											end
											editModeState:set(false)
										end,
										onCancel = function()
											editModeState:set(false)
										end,
									},
								},
							},
						} or builtin.Component{
							layout = builtin.BoxLayout{
								orientation = builtin.type.Orientation.Horizontal,
								children = {
									builtin.Component{
										meta = {
											class = "line-name, " .. (params.entity == nil and "transparent, " or ""),
										},
										layout = builtin.BoxLayout{
											orientation = builtin.type.Orientation.Horizontal,
											children = {
												builtin.TextView{
													meta = {
														class = "font-scale-headline, line-depot-name",
													},
													text = params.nameAndEntityState:old() and params.nameAndEntityState:old().name or nil,
												},
												gui_react_util.makeHorizontalSpacer(),
											},
										},
									},
								},
							},
						} -- Component closing bracket
					},
				}, -- BoxLayout closing bracket
			},
			hoveredState:old() and builtin.Button{
				meta = {
					class = "panel-rename",
					tooltip = params.isLine and _("Rename Line") or _("Rename Depot"),
				},
				content = builtin.ImageView{
					path = params.commonParams.iconPaths.regRename,
					scaling = builtin.type.ImageViewScaling.AutoFit,
				},
				onClick = function()
					if not lockedState:old() then
						editModeState:set(not editModeState:old())
					end
				end,
			} or nil,
		},
	}
end)

local record AutoRenameDialogParams
	commonParams : CommonActionParams
	lineEntities : {Engine.Entity}
	autoRenameSchemeComponentsMapRef : ReactRefT<{string : AutoRename.SchemeComponent}>
	autoRenameSchemesRef : ReactRefT<{AutoRename.Scheme}>
	autoRenameSchemeRef : ReactRefT<AutoRename.Scheme>
	scrollToRenameState : ReactStateT<boolean>
	onClose : function()
end

local AutoRenameDialog = react.RegisterRecipe("AutoRenameDialog", function(params : AutoRenameDialogParams) : TreeNodeId
	if not params.lineEntities then
		return nil
	end

	local textAR = lang_util.format(
		nGetText(
			"Auto-Rename {count} Line",
			"Auto-Rename {count} Lines",
			#params.lineEntities),
		{
			count = lang_util.formatInt(#params.lineEntities),
		}
	)
	local textS = nGetText("Select a Name", "Select Names", #params.lineEntities)
	local textTitle = lang_util.format(_("{autoRename} - {select}"), {
		autoRename = textAR,
		select = textS,
	})

	local autoRenameLinesHeader = builtin.Component{
		meta = {
			class = "auto-rename-lines-header",
		},
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				builtin.TextView{
					meta = {
						class = "font-scale-headline",
					},
					text = textTitle,
				},
			},
		},
	}

	local _nameAndEntityState, commitName : ReactStateT<NameAndEntity>, function(NameAndEntity) = engine_react_util.useStepState(function() : NameAndEntity
		return nil
	end, function(nameAndEntity : NameAndEntity) : Command<EntitySetNameCommandData>, nil
		return api.cmd.makeEntitySetNameCmd(nameAndEntity.entity, nameAndEntity.name)
	end)

	local makeRenameButton = function(scheme : AutoRename.Scheme) : TreeNodeId
		local record Data
			lineEntity : Engine.Entity
			name : string
			color : Vec4f
		end
		local datas : {Data} = {}

		local displayAmount = 3
		local showMore = #params.lineEntities - displayAmount
		for __, line in ipairs(params.lineEntities) do
			local components : {string : string} = {}
			for key, schemeComponentNameRaw in pairs(scheme.formatParams) do
				local sc = params.autoRenameSchemeComponentsMapRef:get()[schemeComponentNameRaw]
				local renameFn = util.useFn(sc.renameFnDefinition.fileName) as function(params : AutoRename.RenameParams) : string

				components[key] = renameFn({
					lineEntity = line,
					reactLine = line_util.getReactLineFromGameState(line),
				})
			end

			local newName = lang_util.format(
				scheme.formatString,
				components
			)

			local lineColor = (api.engine.getComponent(line, api.type.ComponentType.COLOR) as Engine.Component.Color).color

			table.insert(datas, {
				lineEntity = line,
				name = newName,
				color = api.type.Vec4f.new(lineColor.x, lineColor.y, lineColor.z, 1),
			})
		end

		local newNameChildren : {TreeNodeId} = {}
		for i, data in ipairs(datas) do
			if i > displayAmount then
				break
			end

			table.insert(newNameChildren, builtin.Component{
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = {
						builtin.Component{
							meta = {
								styleSheet = styleutil.makeStyle{
									backgroundColor1 = data.color,
								},
								class = "color-dot-view",
							},
						},
						builtin.TextView{
							meta = {
								class = "font-scale-body, auto-rename-name",
							},
							text = data.name,
						},
					},
				},
			})
		end

		local renameButton = builtin.Button{
			meta = {
				class = "auto-rename",
				tooltip = scheme.schemeNameTranslated,
			},
			content = builtin.Component{
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = {
						builtin.ImageView{
							meta = {
								class = "auto-rename-wand",
							},
							path = params.commonParams.iconPaths.regAutoRename,
							scaling = builtin.type.ImageViewScaling.AutoFit,
						},
						builtin.Component{
							layout = builtin.BoxLayout{
								orientation = builtin.type.Orientation.Vertical,
								children = newNameChildren,
							}
						},
					},
				},
			},
			onClick = function()
				for __, data in ipairs(datas) do
					local lineEntity = data.lineEntity
					if api.engine.entityExists(lineEntity) then
						local newName = data.name

						local currentName = api.engine.util.getEntityName(lineEntity)
						if newName ~= currentName then
							commitName({
								name = newName,
								entity = lineEntity,
							})
							if #params.lineEntities == 1 and params.scrollToRenameState then
								params.scrollToRenameState:set(true)
							end
						end
					end
				end

				params.onClose()
			end,
		}

		local tooltipMore = lang_util.format(
			nGetText(
				"Will Change {count} Additional Line",
				"Will Change {count} Additional Lines",
				showMore),
			{
				count = lang_util.formatInt(showMore),
			}
		)
		local textMore = lang_util.format(
			pGetText(
				"auto-rename-additional-lines",
				"+{count}"
			), { count = lang_util.formatInt(showMore), }
		)

		return builtin.Component{
			layout = builtin.FloatingLayout{
				children = {
					builtin.FloatingLayoutChild{
						h = -1,
						v = -1,
						item = renameButton,
					},
					(showMore > 0) and builtin.FloatingLayoutChild{
						h = 1,
						v = 1,
						item = builtin.TextView{
							meta = {
								class = "font-scale-annotation, auto-rename-more",
								tooltip = tooltipMore,
							},
							text = textMore,
						},
					} or nil,
				},
			},
		}
	end

	local children : {TreeNodeId} = {}
	for __, scheme in ipairs(params.autoRenameSchemesRef:get()) do
		table.insert(children, makeRenameButton(scheme))
	end

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			autoRenameLinesHeader,
			builtin.ScrollArea{
				horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
				verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
				content = builtin.Component{
					layout = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Vertical,
						children = children,
					},
				},
			},
		},
	}
end)

local record LinePanelActionButtonsParams
	lineEntities : {Engine.Entity}
	singleLine : Engine.Entity
	depotEntity : Engine.Entity

	autoRenameSchemeComponentsMapRef : ReactRefT<{string : AutoRename.SchemeComponent}>
	autoRenameSchemesRef : ReactRefT<{AutoRename.Scheme}>
	autoRenameSchemeRef : ReactRefT<AutoRename.Scheme>
	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>

	commonParams : CommonActionParams
end

local LinePanelDepotActionButtons = react.RegisterRecipe("LinePanelDepotActionButtons", function(params : LinePanelActionButtonsParams) : TreeNodeId
	local actionListChildrenButtons = {}

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return params.commonParams.moreOptionsState:old()
	end)

	local scrollToRenameState : ReactStateT<boolean> = react.useState(false)
	local oldLineNameState : ReactStateT<string> = react.useState("")
	react.onStep(function()
		if params.depotEntity then
			local currentName = api.engine.util.getEntityName(params.depotEntity)
			if oldLineNameState:old() ~= currentName then
				if scrollToRenameState:old() then
					local ladlApi : LineAndDepotListApi = nil
					if params.commonParams.lineAndDepotListRef:get() then
						ladlApi = params.commonParams.lineAndDepotListRef:get():getApi()
					end

					if ladlApi then
						ladlApi.scrollToRow(params.depotEntity, true)
					end
					scrollToRenameState:set(false)
				end
				oldLineNameState:set(currentName)
			else
			end
		end
	end)

	-- depot name
	local nameAndEntityState: ReactStateT<NameAndEntity> = engine_react_util.useStepState(function() : NameAndEntity
		if not params.depotEntity then
			return nil
		end

		return {
			name = api.engine.util.getEntityName(params.depotEntity),
			entity = params.depotEntity,
		}
	end)

	table.insert(actionListChildrenButtons, builtin.Component{
		meta = { class = "horizontal-spacer" }
	})

	table.insert(actionListChildrenButtons, builtin.Component{
		layout = builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					h = -1,
					v = -1,
					item = builtin.Button(react.ref(params.commonParams.btnBuyVehiclesRef), {
						meta = {
							disableFocusableRecursive = moreOptionsLocalState:old(),
							class = "primary, fixed-size-button, btn-buy-vehicles",
							enabled = params.commonParams.btnBuyVehiclesEnabled(),
							tag = "manager.buyVehicle.button"
						},
						content = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Horizontal,
							children = {
								builtin.ImageView{
									path = params.commonParams.iconPaths.abBuyVehicles,
									scaling = builtin.type.ImageViewScaling.AutoFit,
								},
								builtin.TextView{
									meta = {
										class = "font-scale-body",
									},
									text = _("Buy Vehicles"),
								},
							},
						},
						onClick = function()
							params.commonParams.btnBuyVehiclesOnClick()
						end
					}),
				},
			},
		},
	})


	local actionListChildrenName : { TreeNodeId } = {}

	table.insert(actionListChildrenName, LineOrDepotNameFilling{
		commonParams = params.commonParams,
		entity = params.depotEntity,
		isLine = false,
		nameAndEntityState = nameAndEntityState,
	})

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.Component{
				meta = {
					class = "actions-top",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenButtons,
				}
			},
			builtin.Component{
				meta = {
					class = "actions-splitter",
				},
			},
			builtin.Component{
				meta = {
					class = "actions-bottom",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenName,
				}
			},
			gui_react_util.makeVerticalSpacer(),
		},
	}
end)

local LinePanelActionButtons = react.RegisterRecipe("LinePanelActionButtons", function(params : LinePanelActionButtonsParams) : TreeNodeId
	react.onUnmount(function()
		params.commonParams.gameCtx.windowContainer:get():getApi().removeAllWindows(popover_react_util.PopoverWindow)
	end)

	local actionListChildrenButtons = {}

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return params.commonParams.moreOptionsState:old()
	end)

	local autoRenameOpen : ReactStateT<boolean> = react.useState(false)

	local buttonsEnabled = #params.lineEntities > 0

	local scrollToRenameState : ReactStateT<boolean> = react.useState(false)
	local oldLineNameState : ReactStateT<string> = react.useState("")
	if params.singleLine then
		react.onStep(function()
			if params.singleLine then
				local currentName = api.engine.util.getEntityName(params.singleLine)
				if oldLineNameState:old() ~= currentName then
					if scrollToRenameState:old() then
						local ladlApi : LineAndDepotListApi = nil
						if params.commonParams.lineAndDepotListRef:get() then
							ladlApi = params.commonParams.lineAndDepotListRef:get():getApi()
						end

						if ladlApi then
							ladlApi.scrollToRow(params.singleLine, true)
						end
						scrollToRenameState:set(false)
					end
					oldLineNameState:set(currentName)
				else
				end
			end
		end)
	end

	-- line name
	local nameAndEntityState, _commitName : ReactStateT<NameAndEntity>, function(NameAndEntity) = engine_react_util.useStepState(function() : NameAndEntity
		if not params.singleLine then
			return nil
		end

		return {
			name = api.engine.util.getEntityName(params.singleLine),
			entity = params.singleLine,
		}
	end, function(nameAndEntity : NameAndEntity) : Command<EntitySetNameCommandData>, nil
		return api.cmd.makeEntitySetNameCmd(nameAndEntity.entity, nameAndEntity.name)
	end)

	table.insert(actionListChildrenButtons, builtin.Component{
		meta = { class = "horizontal-spacer" }
	})

	local tooltipAutoRename = nGetText("Auto-Rename Line", "Auto-Rename Lines", #params.lineEntities)

	local lockedState = react.useState(false)

	react.onMouseEvent(function (evt :  Gui.Mouse.Event) : boolean
		if evt.type == api.gui.mouse.Event.Type.Pressed then
			if autoRenameOpen:old() then
				lockedState:set(true)
			end
		elseif evt.type == api.gui.mouse.Event.Type.Released then
			lockedState:set(false)
		end
		return false
	end)

	table.insert(actionListChildrenButtons, button_react_util.makeIconToggleButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abAutoRename,
		function(val : boolean)
			if val and not lockedState:old() then
				autoRenameOpen:set(true)

				lockedState:set(false)

				local pos = params.commonParams.configureLinesPopoverPosRef:get():getPosition(1.0, 1.0)
				local wc = params.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(popover_react_util.PopoverWindow)
				local windowClass = "configure-lines, management"
				wc.addWindow(popover_react_util.PopoverWindow, windowClass, {
					onClose = function()
						wc.removeAllWindows(popover_react_util.PopoverWindow)

						autoRenameOpen:set(false)
						lockedState:set(false)
					end,
					x = pos.x,
					y = pos.y,
					windowTitle = tooltipAutoRename,
					windowClass = windowClass,
					recipe = AutoRenameDialog as IRecipe,
					params = {
						commonParams = params.commonParams,
						lineEntities = params.lineEntities,
						autoRenameSchemeComponentsMapRef = params.autoRenameSchemeComponentsMapRef,
						autoRenameSchemesRef = params.autoRenameSchemesRef,
						autoRenameSchemeRef = params.autoRenameSchemeRef,
						scrollToRenameState = scrollToRenameState,
						onClose = function()
							wc.removeAllWindows(popover_react_util.PopoverWindow)

							autoRenameOpen:set(false)
						end,
					},
					inputActionBubbleUpFilter = builtin.type.InputActionBubbleUpFilter.filterOut{
						"IA_CAMPAIGN_MISSION_MENU_OPEN",
					}
				})
			else
				lockedState:set(false)
			end
		end,
		tooltipAutoRename,
		buttonsEnabled,
		nil,
		moreOptionsLocalState:old(),
		autoRenameOpen:old()
	))

	-- line color
	local record ColorAndEntity
		color : Vec3f
		entity : Engine.Entity
	end
	local __colorState, commitColor : ReactStateT<ColorAndEntity> = engine_react_util.useStepState(function() : ColorAndEntity
		if not params.singleLine then
			return nil
		end
		local lColor = (api.engine.getComponent(params.singleLine, api.type.ComponentType.COLOR) as Engine.Component.Color).color

		return {
			color = api.type.Vec3f.new(lColor.x, lColor.y, lColor.z),
			entity = params.singleLine,
		}
	end, function(
		colorAndEntity : ColorAndEntity) : (Command<EntitySetColorCommandData>,
		function(
			EntitySetColorCommandData,
			boolean,
			{{Engine.Entity, Engine.Revision}}
		))

		return api.cmd.makeEntitySetColorCmd(colorAndEntity.entity, colorAndEntity.color), nil
	end)

	local grayscale = api.gui.genericRep.get(api.gui.genericRep.find("::/gui/main/grayscale.gres")).data as GResColorArray
	local colorPalette = color_util.toArray3(table_util.concatenate((api.gui.genericRep.get(api.gui.genericRep.find("::/gui/line_vehicle_mgmt/line_colors.gres")).data as GResColorArray), grayscale))

	local tooltipSetLineColor = nGetText("Set Line Color", "Set Line Colors", #params.lineEntities)

	table.insert(actionListChildrenButtons, builtin.ColorChooserButton{
		meta = {
			disableFocusableRecursive = moreOptionsLocalState:old(),
			tooltip = tooltipSetLineColor,
			class = "vehicle-button, secondary",
			enabled = buttonsEnabled,
		},
		colors = colorPalette,
		image = params.commonParams.iconPaths.abPaint,
		onValueChange = function(color : Vec3f)
			if params.singleLine then
				commitColor({
					color = color,
					entity = params.singleLine,
				})
			else
				params.commonParams.addFeedback(_("Set color for multiple lines?"), "Question", {
					onAccept = function()
						for __, line in ipairs(params.lineEntities) do
							if api.engine.entityExists(line) then
								commitColor({
									color = color,
									entity = line,
								})
							end
						end
					end,
					acceptText = _("Set Color"),
				}, 2)
			end
		end,
		resetButton = false,
		innerResetButton = false,
		toolTipPosition = builtin.type.ToolTipPosition.Above,
	})

	table.insert(actionListChildrenButtons, builtin.Component{
		meta = { class = "horizontal-fixed-size-spacer" }
	})

	table.insert(actionListChildrenButtons, builtin.Component{
		layout = builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					h = -1,
					v = -1,
					item =builtin.Button(react.ref(params.commonParams.btnBuyVehiclesRef), {
						meta = {
							disableFocusableRecursive = moreOptionsLocalState:old(),
							class = "primary, fixed-size-button, btn-buy-vehicles",
							enabled = params.commonParams.btnBuyVehiclesEnabled(),
							tag = "manager.buyVehicle.button"
						},
						content = builtin.BoxLayout{
							orientation = builtin.type.Orientation.Horizontal,
							children = {
								builtin.ImageView{
									path = params.commonParams.iconPaths.abBuyVehicles,
									scaling = builtin.type.ImageViewScaling.AutoFit,
								},
								builtin.TextView{
									meta = {
										class = "font-scale-body",
									},
									text = _("Buy Vehicles"),
								},
							},
						},
						onClick = function()
							params.commonParams.btnBuyVehiclesOnClick()
						end
					}),
				},
			},
		},
	})


	local actionListChildrenName : { TreeNodeId } = {}

	if params.singleLine then
		table.insert(actionListChildrenName, LineOrDepotNameFilling{
			commonParams = params.commonParams,
			entity = params.singleLine,
			isLine = true,
			nameAndEntityState = nameAndEntityState,
		})
	end

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.Component{
				meta = {
					class = "actions-top",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenButtons,
				}
			},
			builtin.Component{
				meta = {
					class = "actions-splitter",
				},
			},
			params.singleLine and builtin.Component{
				meta = {
					class = "actions-bottom",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenName,
				}
			} or nil,
		},
	}
end)

local LinePanelPlaceholderActionButtons = react.RegisterRecipe("LinePanelPlaceholderActionButtons", function(params : LinePanelActionButtonsParams) : TreeNodeId
	local actionListChildrenButtons = {}

	table.insert(actionListChildrenButtons, builtin.Component{
		meta = { class = "horizontal-spacer" }
	})

	table.insert(actionListChildrenButtons, builtin.Button{
		meta = {
			class = "primary, fixed-size-button",
			enabled = false,
		},
		content = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				builtin.ImageView{
					path = params.commonParams.iconPaths.abBuyVehicles,
					scaling = builtin.type.ImageViewScaling.AutoFit,
				},
				builtin.TextView{
					meta = {
						class = "font-scale-body",
					},
					text = _("Buy Vehicles"),
				},
			},
		},
		onClick = function()
		end
	})


	local actionListChildrenName : { TreeNodeId } = {}

	table.insert(actionListChildrenName, builtin.Component{
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				builtin.Component{
					meta = {
						class = "line-name, transparent",
						forceFocusable = (api.util.getInputMode() ~= api.type["enum"].InputMode.Gamepad),
					},
					layout = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Horizontal,
						children = {
							gui_react_util.makeHorizontalSpacer(),
							builtin.TextView{
								meta = {
									class = "font-scale-headline",
								},
								text = nil,
							},
							gui_react_util.makeHorizontalSpacer(),
						},
					},
				},
			},
		},
	})

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.Component{
				meta = {
					class = "actions-top",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenButtons,
				}
			},
			builtin.Component{
				meta = {
					class = "actions-splitter",
				},
			},
			builtin.Component{
				meta = {
					class = "actions-bottom",
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = actionListChildrenName,
				}
			},
		},
	}
end)

local makeSplitter = function(extraSpace : boolean) : TreeNodeId
	return builtin.Component{
		meta = {
			class = "statistics-splitter, " .. (extraSpace and "extra-space" or "regular"),
		},
	}
end

local record LineStatisticDetailsParams
	showHeader : boolean
	lineEntity : Engine.Entity
	vehicleManagerRef : ReactRefWrapApiT<VehicleManagerParams, VehicleManagerApi>
	lineAndDepotListRef : ReactRefWrapApiT<LineAndDepotListParams, LineAndDepotListApi>
	creatingNewLineState : ReactStateT<boolean>
end

local LineStatisticDetails = react.RegisterRecipe("LineStatisticDetails", function(params : LineStatisticDetailsParams) : TreeNodeId
	if not params.lineEntity then
		return nil
	end

	local lineNameState : ReactStateT<string> = engine_react_util.useStepStateTimer(function() : string
		local nameComp = api.engine.util.getEntityName(params.lineEntity)
		if nameComp then
			return nameComp
		end
		return ""
	end)
	local makeHeader = function() : TreeNodeId
		return builtin.Component{
			meta = {
				class = "line-header",
			},
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					line_react_util.LineCargoDisplay{
						lineEntity = params.lineEntity,
					},
					builtin.Button{
						meta = {
							tooltip = _("Select Line"),
						},
						content = builtin.TextView{
							meta = {
								class = "font-scale-body",
							},
							text = lineNameState:old()
						},
						onClick = function()
							local vmApi : VehicleManagerApi = nil
							if params.vehicleManagerRef:get() then
								vmApi = params.vehicleManagerRef:get():getApi()
							end
							local ladlApi : LineAndDepotListApi = nil
							if params.lineAndDepotListRef:get() then
								ladlApi = params.lineAndDepotListRef:get():getApi()
							end

							if vmApi and ladlApi then
								selectSingleLineAndOverrideSelectionOnClick(
									vmApi,
									ladlApi,
									params.creatingNewLineState,
									params.lineEntity,
									false,
									nil
								)
							end
						end,
					},
				},
			}
		}
	end


	local makeTitleText = function(text : string, tooltip : string) : TreeNodeId
		return builtin.TextView{
			meta = {
				class = "font-scale-body, details-title",
				tooltip = tooltip,
			},
			text = text,
		}
	end

	local makeValueText = function(text : string) : TreeNodeId
		return builtin.TextView{
			meta = {
				class = "font-scale-body, details-value",
			},
			text = text,
		}
	end

	local makeDetailsVertical = function(
		title : string,
		tooltip : string,
		details : TreeNodeId,
		class : string
	) : TreeNodeId
		return builtin.Component{
			meta = {
				class = "details, " .. class,
			},
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Vertical,
				children = {
					makeTitleText(title, tooltip),
					details,
				},
			}
		}
	end

	local makeDetailsSpacer = function() : TreeNodeId
		return builtin.Component{
			meta = {
				class = "details-spacer",
			},
		}
	end


	local detailsChildren : {TreeNodeId} = {}

	-- capacity
	local makeDemandState = function() : integer
		return statistics_react_util.calculateCargoColumnDataForLine(params.lineEntity).demand
	end
	local demandState : ReactStateT<integer> = engine_react_util.useStepState(makeDemandState)

	table.insert(detailsChildren, makeDetailsVertical(
		_("Capacity"),
		_("Maximum load of all vehicles combined."),
		makeValueText(lang_util.format(_("{count}"), {
			count = lang_util.formatInt(demandState:old())
		})),
		"capacity"
	))
	table.insert(detailsChildren, makeDetailsSpacer())

	-- utilization
	local makeCoverageState = function() : number
		return statistics_react_util.calculateCargoColumnDataForLine(params.lineEntity).coverage
	end
	local coverageState : ReactStateT<number> = engine_react_util.useStepStateTimer(makeCoverageState)

	table.insert(detailsChildren, makeDetailsVertical(
		_("Utilization"),
		_("Percentage of the capacity that is currently used."),
		makeValueText(
			api.util.toStringPercentPrecision(coverageState:old(), 0)
		),
		"utilization"
	))
	table.insert(detailsChildren, makeDetailsSpacer())

	-- frequency
	local makeFrequencyState = function() : integer
		return line_util.calculateFrequencySeconds(params.lineEntity)
	end
	local frequencyState : ReactStateT<integer> = engine_react_util.useStepStateTimer(makeFrequencyState)

	table.insert(detailsChildren, makeDetailsVertical(
		_("Frequency"),
		_("Time interval between the vehicles."),
		makeValueText(
			api.util.formatMinutesSeconds(frequencyState:old())
		),
		"frequency"
	))
	table.insert(detailsChildren, makeDetailsSpacer())

	-- rate
	local makeRateState = function() : integer
		return api.engine.util.line.calcLineStationThroughput(params.lineEntity)
	end
	local rateState : ReactStateT<integer> = engine_react_util.useStepStateTimer(makeRateState)

	table.insert(detailsChildren, makeDetailsVertical(
		_("Rate"),
		_("Amount of cargo or passengers a line can transport per year."),
		gui_react_util.integer2TextLayout(rateState:old(), "menu.statistics.line.rateCell"),
		"rate"
	))
	table.insert(detailsChildren, makeDetailsSpacer())

	-- balance
	table.insert(detailsChildren, makeDetailsVertical(
		_("Balance"),
		_("Annual profit or loss."),
		line_react_util.LineBalance{
			entity = params.lineEntity,
			stepTimer = true
		},
		"balance"
	))
	-- no spacer at end!


	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			params.showHeader and makeHeader() or makeSplitter(false),
			builtin.Component{
				meta = {
					class = "statistics-details",
					disableFocusableRecursive = true,
				},
				layout = builtin.BoxLayout{
					orientation = builtin.type.Orientation.Horizontal,
					children = detailsChildren,
				},
			},
			params.showHeader and makeSplitter(true) or nil,
		},
	}
end)

local record DepotStatisticDetailsParams
	depotEntity : Engine.Entity
	vehicleManagerRef : ReactRefWrapApiT<VehicleManagerParams, VehicleManagerApi>
	lineAndDepotListRef : ReactRefWrapApiT<LineAndDepotListParams, LineAndDepotListApi>
	creatingNewLineState : ReactStateT<boolean>
end

local DepotStatisticDetails = react.RegisterRecipe("DepotStatisticDetails", function(params : DepotStatisticDetailsParams) : TreeNodeId
	if not params.depotEntity then
		return nil
	end

	local lineNameState : ReactStateT<string> = engine_react_util.useStepStateTimer(function() : string
		local nameComp = api.engine.util.getEntityName(params.depotEntity)
		if nameComp then
			return nameComp
		end
		return ""
	end)
	local makeHeader = function() : TreeNodeId
		return builtin.Component{
			meta = {
				class = "depot-header",
			},
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					DepotIconDisplay{
						depotEntity = params.depotEntity,
					},
					builtin.Button{
						meta = {
							tooltip = _("Select Depot"),
						},
						content = builtin.TextView{
							meta = {
								class = "font-scale-body",
							},
							text = lineNameState:old()
						},
						onClick = function()
							local vmApi : VehicleManagerApi = nil
							if params.vehicleManagerRef:get() then
								vmApi = params.vehicleManagerRef:get():getApi()
							end
							local ladlApi : LineAndDepotListApi = nil
							if params.lineAndDepotListRef:get() then
								ladlApi = params.lineAndDepotListRef:get():getApi()
							end

							if vmApi and ladlApi then
								selectSingleDepotAndOverrideSelectionOnClick(
									vmApi,
									ladlApi,
									params.creatingNewLineState,
									params.depotEntity,
									false
								)
							end
						end,
					},
				},
			}
		}
	end

	return builtin.BoxLayout{
		children = {
			makeHeader(),
			makeSplitter(true),
		},
	}
end)

local record LineStatisticDetailsUserParams
	vehicleManagerRef : ReactRefWrapApiT<VehicleManagerParams, VehicleManagerApi>
	lineAndDepotListRef : ReactRefWrapApiT<LineAndDepotListParams, LineAndDepotListApi>
	creatingNewLineState : ReactStateT<boolean>
end

local LineStatisticDetailsEntry = react.RegisterRecipe("LineStatisticDetailsEntry", function(params : Builtin.TableCellParam) : TreeNodeId
	local entity = params.rowKey
	if not api.engine.entityExists(entity) then
		return nil
	end

	local line : Engine.Component.Line = api.engine.getComponent(entity, api.type.ComponentType.LINE)
	local isDepot : boolean = (line == nil) and true or false

	local userParams = params.userParam as LineStatisticDetailsUserParams

	return builtin.BoxLayout{
		child = isDepot and DepotStatisticDetails{
			depotEntity = entity,
			vehicleManagerRef = userParams.vehicleManagerRef,
			lineAndDepotListRef = userParams.lineAndDepotListRef,
			creatingNewLineState = userParams.creatingNewLineState,
		} or LineStatisticDetails{
			showHeader = true,
			lineEntity = entity,
			vehicleManagerRef = userParams.vehicleManagerRef,
			lineAndDepotListRef = userParams.lineAndDepotListRef,
			creatingNewLineState = userParams.creatingNewLineState,
		},
	}
end)

local record LineStatisticDetailsListParams
	linesSelected : {EntityUtil.EntityAndRevision}
	depotsSelected : {EntityUtil.EntityAndRevision}
	commonParams : CommonActionParams
end

local LineStatisticDetailsList = react.RegisterRecipe("LineStatisticDetailsList", function(params : LineStatisticDetailsListParams) : TreeNodeId
	local record TableState
		rowKeys : {Engine.Entity}
	end
	local tableState : ReactStateT<TableState> = engine_react_util.useStepState(function() : TableState
		local rowKeys : {Engine.Entity} = {}

		local allPlayerLines = api.engine.system.lineSystem.getLinesForPlayer(api.engine.util.getPlayer())
		local depotKeys : {Engine.Entity} = api.engine.getEntitiesWithComponent(api.type.ComponentType.VEHICLE_DEPOT, {
			requireOwnedByPlayer = api.engine.util.getPlayer(),
			filterFn = function(entity : Engine.Entity) : boolean
				local vehicleDepot : Engine.Component.VehicleDepot = api.engine.getComponent(entity, api.type.ComponentType.VEHICLE_DEPOT)
				return vehicleDepot and (#vehicleDepot.inNodes > 0 or #vehicleDepot.outNodes > 0)
			end,
		})

		for __, lineEAR in ipairs(params.linesSelected) do
			if table_util.arrayContains(allPlayerLines, lineEAR.entity) then
				table.insert(rowKeys, lineEAR.entity)
			end
		end
		for __, depotEAR in ipairs(params.depotsSelected) do
			if table_util.arrayContains(depotKeys, depotEAR.entity) then
				table.insert(rowKeys, depotEAR.entity)
			end
		end

		return {
			rowKeys = rowKeys,
		}
	end)

	local columns = {
		builtin.ColumnDesc{
			name = _("Name"),
			recipe = LineStatisticDetailsEntry,
			getCompareValue = function(rowKey : integer) : { boolean, string }
				local line : Engine.Component.Line = api.engine.getComponent(rowKey, api.type.ComponentType.LINE)
				local isDepot : boolean = (line == nil) and true or false

				local name = api.engine.util.getEntityName(rowKey)

				return { isDepot, name }
			end,
			weight = 1.0,
		}
	}

	local userParams : LineStatisticDetailsUserParams = {
		vehicleManagerRef = params.commonParams.vehicleManagerRef,
		lineAndDepotListRef = params.commonParams.lineAndDepotListRef,
		creatingNewLineState = params.commonParams.creatingNewLineState,
	}

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			builtin.DataTable{
				disableSortKey = true,
				columns = columns,
				rowKeys = tableState:old().rowKeys,
				userParam = userParams,
				scrollPolicyVertical = builtin.type.ScrollBarPolicy.AsNeededButAlwaysReserveSpace,
				scrollPolicyHorizontal = builtin.type.ScrollBarPolicy.AlwaysOff,
				keyboardNavigation = true,
			},
		},
	}
end)

local record LinePanelParams
	commonParams : CommonActionParams

	linePanelRef : ReactRefWrap
	linePanelActionButtonsRef : ReactRefWrap
	autoRenameSchemeComponentsMapRef : ReactRefT<{string : AutoRename.SchemeComponent}>
	autoRenameSchemesRef : ReactRefT<{AutoRename.Scheme}>
	autoRenameSchemeRef : ReactRefT<AutoRename.Scheme>
	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>
end

local LinePanel = react.RegisterRecipe("LinePanel", function(params : LinePanelParams) : TreeNodeId
	if not params.commonParams.lineManagerStateRef:get() then
		return nil
	end

	local record LinePanelState
		linesSelected : {EntityUtil.EntityAndRevision}
		depotsSelected : {EntityUtil.EntityAndRevision}
	end
	local linePanelState : ReactStateT<LinePanelState> = engine_react_util.useStepState(function() : LinePanelState
		local linesSelected = params.commonParams.lineManagerStateRef:get().lineListEntitiesSelected
		local depotsSelected = params.commonParams.lineManagerStateRef:get().depotListEntitiesSelected

		return {
			linesSelected = linesSelected,
			depotsSelected = depotsSelected,
		}
	end)

	if not linePanelState:old() then
		return nil
	end

	local linePanelHelper : TreeNodeId = nil
	if (#linePanelState:old().linesSelected == 0) and
		(#linePanelState:old().depotsSelected == 0) and
		(not params.commonParams.creatingNewLineState:old()) then

		linePanelHelper = builtin.Component{
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					gui_react_util.makeHorizontalSpacer(),
					builtin.TextView{
						meta = {
							class = "empty-text, font-scale-body"
						},
						text = (params.commonParams.gamepadInputModeState:old() and
							((params.commonParams.lvm2D3DModeState:old() == "2D") and _("No line or depot selected.") or "")) or
							_("Select a line or depot."),
					},
					gui_react_util.makeHorizontalSpacer(),
				},
			},
		}
	end

	local linePanelActionButtons : TreeNodeId = nil
	local lineOrDepotManagerPanel : TreeNodeId = nil
	local lineStatisticDetails : TreeNodeId = nil
	local lineEntity : Engine.Entity = nil
	if params.commonParams.lineState and params.commonParams.lineState:old() then
		if params.commonParams.lineState:old().entityAndRevision then
			lineEntity = params.commonParams.lineState:old().entityAndRevision.entity
		end
	end

	if (params.commonParams.lineState and params.commonParams.lineState:old()) and
		(#linePanelState:old().linesSelected == 1) and (#linePanelState:old().depotsSelected == 0) then

		linePanelActionButtons = LinePanelActionButtons(react.ref(params.linePanelActionButtonsRef), {
			meta = {
				onFocusChange = function(focused)
					params.commonParams.lvmActionButtonsState:set(focused and "LPLAB" or "NONE")
				end,
			},

			lineEntities = {lineEntity},
			singleLine = lineEntity,

			autoRenameSchemeComponentsMapRef = params.autoRenameSchemeComponentsMapRef,
			autoRenameSchemesRef = params.autoRenameSchemesRef,
			autoRenameSchemeRef = params.autoRenameSchemeRef,
			firstStopToSendSchemesRef = params.firstStopToSendSchemesRef,
			firstStopToSendSchemeMapRef = params.firstStopToSendSchemeMapRef,

			commonParams = params.commonParams,
		})
		lineOrDepotManagerPanel = builtin.ScrollArea{
			content = builtin.Component{
				layout = builtin.BoxLayout{
					child = LineManagerPanel(react.ref(params.linePanelRef), {
						commonParams = params.commonParams,
					}),
				}
			},
			horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
			verticalPolicy = builtin.type.ScrollBarPolicy.AsNeededButAlwaysReserveSpace,
		}
		lineStatisticDetails = LineStatisticDetails{
			showHeader = false,
			lineEntity = lineEntity,
		}
	elseif ((#linePanelState:old().linesSelected == 0) and (#linePanelState:old().depotsSelected == 1)) then
		linePanelActionButtons = LinePanelDepotActionButtons(react.ref(params.linePanelActionButtonsRef), {
			meta = {
				onFocusChange = function(focused)
					params.commonParams.lvmActionButtonsState:set(focused and "LPDAB" or "NONE")
				end,
			},

			depotEntity = linePanelState:old().depotsSelected[1].entity,

			autoRenameSchemeComponentsMapRef = params.autoRenameSchemeComponentsMapRef,
			autoRenameSchemesRef = params.autoRenameSchemesRef,
			autoRenameSchemeRef = params.autoRenameSchemeRef,
			firstStopToSendSchemesRef = params.firstStopToSendSchemesRef,
			firstStopToSendSchemeMapRef = params.firstStopToSendSchemeMapRef,

			commonParams = params.commonParams,
		})
		params.linePanelRef:set(nil)
	elseif #linePanelState:old().linesSelected > 0 or #linePanelState:old().depotsSelected > 0 then
		local lineEntities : {Engine.Entity} = {}
		for __, lineEAR in ipairs(linePanelState:old().linesSelected) do
			if api.engine.entityExists(lineEAR.entity) then
				if entity_util.isOwnedByPlayer(lineEAR.entity) then
					table.insert(lineEntities, lineEAR.entity)
				end
			end
		end

		linePanelActionButtons = LinePanelActionButtons(react.ref(params.linePanelActionButtonsRef), {
			meta = {
				onFocusChange = function(focused)
					params.commonParams.lvmActionButtonsState:set(focused and "LPLAB" or "NONE")
				end,
			},

			lineEntities = lineEntities,
			singleLine = nil, -- case with ONLY a single line selected is handled above

			autoRenameSchemeComponentsMapRef = params.autoRenameSchemeComponentsMapRef,
			autoRenameSchemesRef = params.autoRenameSchemesRef,
			autoRenameSchemeRef = params.autoRenameSchemeRef,
			firstStopToSendSchemesRef = params.firstStopToSendSchemesRef,
			firstStopToSendSchemeMapRef = params.firstStopToSendSchemeMapRef,

			commonParams = params.commonParams,
		})

		lineStatisticDetails = LineStatisticDetailsList(react.ref(params.linePanelRef), {
			linesSelected = linePanelState:old().linesSelected,
			depotsSelected = linePanelState:old().depotsSelected,
			commonParams = params.commonParams,
		})
	else
		params.linePanelRef:set(nil)
		linePanelActionButtons = LinePanelPlaceholderActionButtons(react.ref(params.linePanelActionButtonsRef), {
			meta = {
				onFocusChange = function(focused)
					params.commonParams.lvmActionButtonsState:set(focused and "LPPAB" or "NONE")
				end,
			},
			commonParams = params.commonParams,
		})
		lineStatisticDetails = gui_react_util.makeVerticalSpacer()
	end

	return builtin.FloatingLayout{
		children = {
			builtin.FloatingLayoutChild{
				h = -1,
				v = -1,
				item = builtin.Component{
					layout = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Vertical,
						children = {
							builtin.Component{
								layout = builtin.FloatingLayout{
									children = {
										builtin.FloatingLayoutChild{
											h = -1,
											v = -1,
											item = linePanelActionButtons,
										},
										builtin.FloatingLayoutChild{
											h = 0,
											v = 0,
											item = builtin.Component(react.ref(params.commonParams.configureLinesPopoverPosRef), {
												mouseTransparent = true,
												meta = {
													class = "popover-anchor",
												},
											}),
										},
									},
								},
							},
							linePanelHelper,
							lineOrDepotManagerPanel,
							lineStatisticDetails,
						},
					},
				},
			},
			params.commonParams.creatingNewLineState:old() and builtin.FloatingLayoutChild{
				h = 0.5,
				v = 0,
				item = InfoBox{
					infoTitle = _("Creating New Line"),
					infoMessage = _("Add stations by selecting them in the world to see them here."),
					infoCancelMessage = _("Cancel Creating New Line"),
					commonParams = params.commonParams,
				},
			} or (params.commonParams.gamepadNo3DSelectionInfoState:old() and
					params.commonParams.lvm2D3DModeState:old() == "3D") and builtin.FloatingLayoutChild{
				h = 0.5,
				v = 0,
				item = InfoBox{
					infoTitle = _("Make Selection"),
					infoMessage = _("Select a vehicle, line or depot in the world."),
					commonParams = params.commonParams,
				},
			} or nil,
		},
	}
end)

local getVehicleDataForScheme = function(selectionToLine : {Engine.Entity}) : FirstStopToSend.VehicleData
	local carrier : Carrier = nil
	local supportsPassengers = false

	for __, selectedVehicle in ipairs(selectionToLine) do
		if api.engine.entityExists(selectedVehicle) then
			local tv : Engine.Component.TransportVehicle = api.engine.getComponent(selectedVehicle, api.type.ComponentType.TRANSPORT_VEHICLE)

			if tv then
				if not carrier then
					carrier = tv.carrier
				end
				if vehicle_util.isAbleToLoadPassenger(tv) then
					supportsPassengers = true
				end
			end
		end
	end

	return {
		carrier = carrier,
		supportsPassengers = supportsPassengers,
	}
end

local getSendScheme = function(
	selectionToLine : {Engine.Entity},
	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>,
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>
) : FirstStopToSend.Scheme
	local vehicleData = getVehicleDataForScheme(selectionToLine)

	local sendSchemes : FirstStopToSend.Schemes = vehicleData.carrier and firstStopToSendSchemeMapRef:get()[vehicleData.carrier] or nil
	local sendSchemeString : string = "nextreachable"
	if not sendSchemes then -- fallback
		sendSchemeString = vehicleData.supportsPassengers and "nextreachable" or "firststop"
	else
		sendSchemeString = vehicleData.supportsPassengers and sendSchemes.supportsPassengers or sendSchemes.cargoOnly
	end

	local sendScheme : FirstStopToSend.Scheme = nil
	for __, scheme in ipairs(firstStopToSendSchemesRef:get()) do
		if scheme.schemeNameRaw == sendSchemeString then
			sendScheme = scheme
			break
		end
	end

	if not sendScheme then
		sendScheme = firstStopToSendSchemesRef:get()[1]
	end

	return sendScheme
end

local record SendToLineSelectionParams
	commonParams : CommonActionParams
	selection : {Engine.Entity}
	sendToLine : function(lineEntity : Engine.Entity, sendScheme : FirstStopToSend.Scheme)
	onlyVisible : boolean
	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>
end

local SendToLineSelection = react.RegisterRecipe("SendToLineSelection",
		function(params : SendToLineSelectionParams) : TreeNodeId
	local onlyVisibleState = react.useState(params.onlyVisible) as ReactStateT<boolean>

	local record LineData
		lineEntity : Engine.Entity
		lineName : string
		lineColor : Vec4f
	end
	local lineDatasState = engine_react_util.useStepState(function() : {LineData}
		local filteredLines = makeFilteredLinesForSendToLine(
			params.selection,
			onlyVisibleState:old()
		)

		local filteredLineDatas : {LineData} = {}
		for __, lineEntity in ipairs(filteredLines) do
			local lineColor = (api.engine.getComponent(lineEntity, api.type.ComponentType.COLOR) as Engine.Component.Color).color

			table.insert(filteredLineDatas, {
				lineEntity = lineEntity,
				lineName = api.engine.util.getEntityName(lineEntity),
				lineColor = api.type.Vec4f.new(lineColor.x, lineColor.y, lineColor.z, 1)
			})
		end

		table.sort(filteredLineDatas, function(a : LineData, b : LineData) : boolean
			return lang_util.compareStrings(a.lineName, b.lineName) < 0
		end)

		return filteredLineDatas
	end)

	local value2firstStopToSendSchemes : {string : FirstStopToSend.Scheme} = {}
	local firstStopToSendItems : {TreeNodeId} = {}
	for __, scheme in ipairs(params.firstStopToSendSchemesRef:get()) do
		value2firstStopToSendSchemes[scheme.schemeNameRaw] = scheme
		table.insert(firstStopToSendItems, builtin.ComboBoxItem{
			value = scheme.schemeNameRaw,
			content = builtin.TextView{
				meta = {
					class = "font-scale-annotation",
				},
				text = scheme.schemeNameTranslated,
			},
		})
	end

	local sendSchemeInitial = getSendScheme(
		params.selection,
		params.firstStopToSendSchemesRef,
		params.firstStopToSendSchemeMapRef
	)

	local sendHeader = builtin.Component{
		meta = {
			class = "send-header",
			forceFocusable = true,
		},
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				builtin.ToggleButton{
					meta = {
						class = "only-visible",
						tooltip = _("Show Only Visible Lines"),
					},
					content = builtin.ImageView{
						path = params.commonParams.iconPaths.regFilterVisible,
						scaling = builtin.type.ImageViewScaling.AutoFit,
					},
					onValueChange = function(val : integer)
						onlyVisibleState:set(val == 1 and true or false)
					end,
					value = onlyVisibleState:old() and 1 or 0
				},
				builtin.TextView{
					meta = {
						class = "send-title, font-scale-headline",
					},
					text = _("Send to Line"),
				},
				gui_react_util.makeHorizontalSpacer(),
				builtin.ComboBox{
					meta = {
						tooltip = _("Stop To Send Vehicles To"),
					},
					value = sendSchemeInitial.schemeNameRaw,
					onValueChange = function(value : string)
						local vehicleData = getVehicleDataForScheme(params.selection)
						if vehicleData.carrier ~= nil then
							local schemeMap = params.firstStopToSendSchemeMapRef:get()
							if vehicleData.supportsPassengers then
								schemeMap[vehicleData.carrier].supportsPassengers = value
							else
								schemeMap[vehicleData.carrier].cargoOnly = value
							end
							params.firstStopToSendSchemeMapRef:set(schemeMap)
						end
					end,
					items = firstStopToSendItems,
					compact = true,
				},
			},
		},
	}

	react.useInputAction("IA_NAV_LEFT", react.iaFocusTraversal(true, false))
	react.useInputAction("IA_NAV_RIGHT", react.iaFocusTraversal(true, true))
	react.useInputAction("IA_NAV_UP", react.iaFocusTraversal(false, false))
	react.useInputAction("IA_NAV_DOWN", react.iaFocusTraversal(false, true))

	local children = {}
	for __, lineData in ipairs(lineDatasState:old()) do
		table.insert(children, builtin.Button{
			meta = {
				class = "line-button",
				onMouseOver = function(hover : boolean)
					if hover then
						params.commonParams.sendToLineHoveredLineState:set(lineData.lineEntity)
					else
						params.commonParams.sendToLineHoveredLineState:set(nil)
					end
				end,
			},
			content = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					builtin.Component{
						meta = {
							styleSheet = styleutil.makeStyle{
								backgroundColor1 = lineData.lineColor
							},
							class = "color-dot-view",
						},
					},
					builtin.TextView{
						meta = {
							class = "font-scale-body",
						},
						text = lineData.lineName
					}
				}
			},
			onClick = function()
				if api.engine.entityExists(lineData.lineEntity) then
					local sendScheme = getSendScheme(
						params.selection,
						params.firstStopToSendSchemesRef,
						params.firstStopToSendSchemeMapRef
					)
					params.sendToLine(lineData.lineEntity, sendScheme)
				end
			end,
		})
	end

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			sendHeader,
			builtin.ScrollArea{
				horizontalPolicy = builtin.type.ScrollBarPolicy.AlwaysOff,
				verticalPolicy = builtin.type.ScrollBarPolicy.AsNeeded,
				content = builtin.Component{
					layout = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Vertical,
						children = children,
					},
				},
			},
		}
	}
end)

local sendVehiclesToLineCommands = function(
	selectionToLine : {Engine.Entity},
	lineEntity : Engine.Entity,
	firstStopToSendScheme : FirstStopToSend.Scheme,
	afterSendCallback : function(sentEntities : {Engine.Entity}),
	bufferedSendDataRef : ReactRefT<{BufferedLVMSendData}>,
	doNotBuffer : boolean
)
	local sendFn = util.useFn(firstStopToSendScheme.firstStopToSendFnDefinition.fileName) as function(params : FirstStopToSend.SendParams) : {Engine.Entity : integer}

	local vehicle2stopIndex : {Engine.Entity : integer} = sendFn({
		vehicleEntities = selectionToLine,
		lineEntity = lineEntity
	})

	for vehicleEntity, stopIndex in orderedPairs(vehicle2stopIndex) do
		local sendVehicleToLineCallback = function(result : VehicleSetLineCommandData, success : boolean)
			log.verbose("Command send vehicle to line: " .. (success and "success" or "fail"))
			if not success then
				local failReason = api.engine.util.line.getFailedPathReason(
					result.vehicleEntity,
					result.lineEntity,
					true -- fullLine
				)
				local pos = failReason.pos
				react.fireEvent(
					nil,
					"cannotAssignToLine",
					{
						reason = failReason.reason,
						pos = pos and { x = pos.x, y = pos.y, z = pos.z } or nil
					}
				)
			end

			if success then
				api.gui.sound.playRandomSoundEffect(lvmSFX.VehicleSetLine)
			end

			if api.engine.entityExists(result.vehicleEntity) and api.engine.entityExists(result.lineEntity) then
				if afterSendCallback then
					local selection : {Engine.Entity} = {}
					if success then
						selection = {result.vehicleEntity}
					end
					if success or doNotBuffer then
						afterSendCallback(selection)
					end
				end

				if not success then
					local data = bufferedSendDataRef:get()
					if not bufferedSendDataRef:hasExpired() and data then
						table.insert(data, {
							line = result.lineEntity,
							vehicle = result.vehicleEntity,
							stopToSend = stopIndex,
						})
						bufferedSendDataRef:set(data)
					end
				end
			end
		end

		api.cmd.sendCommand(
			api.cmd.makeVehicleSetLineCmd(
				vehicleEntity as Engine.Entity,
				lineEntity,
				stopIndex
			),
			sendVehicleToLineCallback,
			nil -- progress
		)

	end
end

local cleanupSendToLine = function(
	gameCtx : GameContext,
	sendToLineDisabledState : ReactStateT<boolean>,
	switchToPrevMode : function()
)
	sendToLineDisabledState:set(false)

	local wc : Builtin.WindowAPI = gameCtx.windowContainer:get():getApi()
	wc.removeAllWindows(popover_react_util.PopoverWindow)

	switchToPrevMode()
end

local record SendToLineButtonParams
	commonParams : CommonActionParams

	vehicleEARSelection: {EntityUtil.EntityAndRevision}

	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>

	sendToLineDisabledState : ReactStateT<boolean>
	focusedLastState : ReactStateT<boolean>

	afterSendingToLineFixup : function(successfullySentVehicles : {Engine.Entity})
end

local SendToLineButton = react.RegisterRecipe("SendToLineButton", function(params : SendToLineButtonParams) : TreeNodeId
	local function getWindowApi() : Builtin.WindowAPI
		return params.commonParams.gameCtx.windowContainer:get():getApi()
	end

	react.onUnmount(function()
		getWindowApi().removeAllWindows(popover_react_util.PopoverWindow)
		params.sendToLineDisabledState:set(false)
	end)

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return params.commonParams.moreOptionsState:old()
	end)

	if not moreOptionsLocalState:old() then
		react.useInputAction("IA_SELECT", react.iaForward(params.commonParams.btnSendToLineRef))
	end

	return builtin.FloatingLayout{
		children = {
			builtin.FloatingLayoutChild{
				h = -1,
				v = -1,
				item = builtin.Button(react.ref(params.commonParams.btnSendToLineRef), {
					meta = {
						disableFocusableRecursive = moreOptionsLocalState:old(),
						class = "primary, fixed-size-button, btn-send-to-line," ..
							(params.commonParams.btnSendToLineEnabled() and "" or "keyhint-hidden"), -- only this button seems to need to be told that it should not show a keyhint when disabled
						enabled = params.commonParams.btnSendToLineEnabled(),
						id = "menu.management.vehicle.sendToLineButton",
						onFocusChange = function(focus : boolean)
							params.focusedLastState:set(focus)
						end,
					},
					content = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Horizontal,
						children = {
							builtin.ImageView {
								path = params.commonParams.iconPaths.abSendToLine,
								scaling = builtin.type.ImageViewScaling.AutoFit,
							},
							builtin.TextView{
								meta = {
									class = "font-scale-body",
								},
								text = _("Send to Line"),
							},
						},
					},
					onClick = function(__value : integer)
						params.commonParams.btnSendToLineOnClick()
					end
				}),
			},
		},
	}
end)

local record VehicleManagerActionButtonsParams
	firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}>
	firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}>

	sendToLineDisabledState : ReactStateT<boolean>
	vehicleStoreEventParamState : ReactStateT<VehicleStoreWindow.VehicleStoreEventParam>

	commonParams : CommonActionParams

	vehicleManagerApi : VehicleManagerApi

	afterSendingToLineFixup : function(sentEntities : {Engine.Entity})
end

local VehicleManagerActionButtons = react.RegisterRecipe("VehicleManagerActionButtons", function(params : VehicleManagerActionButtonsParams) : TreeNodeId
	local firstRef = react.useNodeRef()
	local focusedFirstState : ReactStateT<boolean> = react.useState(false)
	local focusedLastState : ReactStateT<boolean> = react.useState(false)
	if focusedFirstState:old() then
		react.useInputAction("IA_NAV_LEFT", react.iaHandler(function()
			if params.commonParams.btnSendToLineRef:get() then
				params.commonParams.btnSendToLineRef:get():focus()
			end
		end))
	end
	if focusedLastState:old() then
		react.useInputAction("IA_NAV_RIGHT", react.iaHandler(function()
			if firstRef:get() then
				firstRef:get():focus()
			end
		end))
	end

	local mirroredVehicleManagerState = react.useMirrorState(params.commonParams.vehicleManagerStateRef)

	local moreOptionsLocalState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return params.commonParams.moreOptionsState:old()
	end)

	local vehicleEARSelection : { EntityUtil.EntityAndRevision } = mirroredVehicleManagerState:old().vehicleListEntitiesSelected

	local record VehicleColoringConfig
		vehiclesWithoutCblend : {Engine.Entity : boolean}
		disabled : boolean
	end
	local vehicleColoringConfigState : ReactStateT<VehicleColoringConfig> = engine_react_util.useStepState(function() : VehicleColoringConfig
		local function canBeColored(entity : Engine.Entity) : boolean
			local transportVehicle = api.engine.getComponent(entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
			for _, vehicle in ipairs(transportVehicle.transportVehicleConfig.vehicles) do
				local tv = api.res.modelRep.get(vehicle.part.modelId).metadata.transportVehicle as ModelMetadata.TransportVehicle
				if not tv.noCblendMask then
					return true
				end
			end
			return false
		end

		local vehiclesWithoutCblend : {Engine.Entity : boolean} = {}
		local disableButton = true
		for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
			if canBeColored(vehicleEAR.entity) then
				disableButton = false
			else
				vehiclesWithoutCblend[vehicleEAR.entity] = true
			end
		end

		return {
			vehiclesWithoutCblend = vehiclesWithoutCblend,
			disabled = disableButton,
		}
	end)

	local ratio = params.commonParams.countDataState:old().selectionRatioVehicles
	local countSelected = params.commonParams.countDataState:old().countVehiclesSelected
	local countVehicleProblems = params.commonParams.countDataState:old().countVehiclesWithProblems

	local vehicleSelectionValue = ratio
	if vehicleSelectionValue ~= 0 and vehicleSelectionValue ~= 1 then
		vehicleSelectionValue = -1
	end

	local actionListChildren = {}

	local allVehiclesCheckbox = builtin.CheckBox(react.ref(firstRef), {
		meta = {
			disableFocusableRecursive = moreOptionsLocalState:old(),
			enabled = (
				#mirroredVehicleManagerState:old().vehicleListEntitiesSelected ~= 0
				or #mirroredVehicleManagerState:old().vehicleListEntitiesUnselected ~= 0
			),
			tooltip = makeVehicleProblemsTooltip(countVehicleProblems),
			onFocusChange = function(focus : boolean)
				focusedFirstState:set(focus)
			end,
		},
		onValueChange = function(__val : integer)
			local vehicleListEntities : {EntityUtil.EntityAndRevision} = {}
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				table.insert(vehicleListEntities, vehicleEAR)
			end
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected) do
				table.insert(vehicleListEntities, vehicleEAR)
			end

			if ratio == 1 then
				params.commonParams.vehicleManagerStateRef:set(
					makeVehicleManagerState({}, vehicleListEntities)
				)
			else
				params.commonParams.vehicleManagerStateRef:set(
					makeVehicleManagerState(vehicleListEntities, {})
				)
			end
		end,
		value = vehicleSelectionValue,
	})

	table.insert(actionListChildren, makeCheckboxWithProblemCount(
		allVehiclesCheckbox,
		countVehicleProblems
	))

	table.insert(actionListChildren, builtin.Component{
		meta = { class = "horizontal-fixed-size-spacer" }
	})

	local tooltipReverse : string = nil
	if ratio == 1 then
		tooltipReverse = _("Reverse All Vehicles")
	else
		tooltipReverse = nGetText("Reverse Selected Vehicle", "Reverse Selected Vehicles", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abReverse,
		function()
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				api.cmd.sendCommand(api.cmd.makeVehicleReverseCmd(vehicleEAR.entity),
					function(_result : ICommandData, _success : boolean)
					end)
			end
		end,
		tooltipReverse,
		#vehicleEARSelection > 0,
		nil,
		moreOptionsLocalState:old()
	))

	local allVehiclesStoppedState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		local allStopped = true
		for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
			local tv = api.engine.getComponent(vehicleEAR.entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
			if not tv.userStopped then
				allStopped = false
				break
			end
		end
		return allStopped
	end)

	local tooltipStartStop = ""
	if allVehiclesStoppedState:old() then
		if ratio == 1 then
			tooltipStartStop = _("Start All Vehicles")
		else
			tooltipStartStop = nGetText("Start Selected Vehicle", "Start Selected Vehicles", countSelected)
		end
	else
		if ratio == 1 then
			tooltipStartStop = _("Stop All Vehicles")
		else
			tooltipStartStop = nGetText("Stop Selected Vehicle", "Stop Selected Vehicles", countSelected)
		end
	end

	table.insert(actionListChildren, button_react_util.makeIconButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abStartStop,
		function()
			local newState = not allVehiclesStoppedState:old()

			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				local tv = api.engine.getComponent(vehicleEAR.entity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
				if tv.userStopped ~= newState then
						api.cmd.sendCommand(api.cmd.makeVehicleSetStoppedByUserCmd(vehicleEAR.entity, newState),
							function(_result : ICommandData, _success : boolean)
							end)
				end
			end
		end,
		tooltipStartStop,
		#vehicleEARSelection > 0,
		"menu.management.startStop",
		moreOptionsLocalState:old()
	))

	local tooltipColorVehicles : string = nil
	if ratio == 1 then
		tooltipColorVehicles = _("Set Color for All Vehicles")
	else
		tooltipColorVehicles = nGetText("Set Color for Selected Vehicle", "Set Color for Selected Vehicles", countSelected)
	end

	local grayscale = api.gui.genericRep.get(api.gui.genericRep.find("::/gui/main/grayscale.gres")).data as GResColorArray
	local colorPalette = color_util.toArray3(table_util.concatenate((api.gui.genericRep.get(api.gui.genericRep.find("::/gui/line_vehicle_mgmt/line_colors.gres")).data as GResColorArray), grayscale))

	table.insert(actionListChildren,
		builtin.ColorChooserButton {
			meta = {
				disableFocusableRecursive = moreOptionsLocalState:old(),
				class = "vehicle-button, secondary",
				enabled = #vehicleEARSelection > 0 and not vehicleColoringConfigState:old().disabled,
				tooltip = tooltipColorVehicles,
			},
			image = params.commonParams.iconPaths.abPaint,
			colors = colorPalette,
			onValueChange = function(selectedColor : Vec3f)
				local disabledVehicles = vehicleColoringConfigState:old().vehiclesWithoutCblend
				for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
					if not disabledVehicles[vehicleEAR.entity] then
						api.cmd.sendCommand(api.cmd.makeEntitySetColorCmd(vehicleEAR.entity, selectedColor),
							function(_result : ICommandData, success : boolean)
								log.verbose("Set color command " .. tostring(success))
						end)
					end
				end
			end,
			resetButton = false,
			innerResetButton = true,
		}
	)

	table.insert(actionListChildren, builtin.Component{
		meta = {
			enabled = false,
			disableFocusableRecursive = true,
			class = "horizontal-spacer",
		}
	})

	local tooltipClone : string = nil
	if ratio == 1 then
		tooltipClone = _("Clone All Vehicles")
	else
		tooltipClone = nGetText("Clone Selected Vehicle", "Clone Selected Vehicles", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconButton(
		"vehicle-button, buy-sfx, secondary",
		params.commonParams.iconPaths.abDuplicate,
		function()
			local vehicleEntities : {Engine.Entity} = {}
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				table.insert(vehicleEntities, vehicleEAR.entity)
			end

			local duplicateVehiclesParam : DuplicateVehiclesParam = {
				vehicleEntities = vehicleEntities,
				addFeedback = params.commonParams.addFeedback,
				onBuy = function(resultEntities : {{Engine.Entity, Engine.Revision}})
					for __, v in ipairs(resultEntities) do
						params.vehicleManagerApi.addVehiclesToVehicleListAndSelect({v[1]})
					end
				end,
			}
			react.fireEvent(nil, "duplicateVehicles", duplicateVehiclesParam)
		end,
		tooltipClone,
		#vehicleEARSelection > 0,
		nil,
		moreOptionsLocalState:old()
	))

	local allSelectedVehiclesInSingleCarrier = function(
		multiVehicleOnly : boolean,
		separateHelicopters : boolean
	) : boolean
		local carriersMap = {}
		local hasAircraft = false
		local hasHelicopters = false

		local tmHelicopter = api.type["enum"].TransportMode.HELICOPTER
		local tmAircraft   = api.type["enum"].TransportMode.AIRCRAFT
		local tmSmAircraft = api.type["enum"].TransportMode.SMALL_AIRCRAFT

		for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
			local tv : Engine.Component.TransportVehicle = api.engine.getComponent(vehicleEAR.entity, api.type.ComponentType.TRANSPORT_VEHICLE)
			carriersMap[tv.carrier] = true

			local transportModeMap = vehicle_store_util.getTransportModeMapForVehicles({vehicleEAR.entity})
			if transportModeMap[tmHelicopter] then
				hasHelicopters = true
			elseif transportModeMap[tmAircraft] or transportModeMap[tmSmAircraft] then
				hasAircraft = true
			end

			if separateHelicopters then
				if hasAircraft and hasHelicopters then
					return false
				end
			end

			if multiVehicleOnly then
				if (tv.carrier ~= api.type["enum"].Carrier.TRAM and tv.carrier ~= api.type["enum"].Carrier.RAIL) then
					return false
				end
			end
		end
		local carrierCount = 0
		for __, __ in pairs(carriersMap) do
			carrierCount = carrierCount + 1
		end
		return carrierCount == 1
	end

	local selectionReplacableState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return allSelectedVehiclesInSingleCarrier(false, true)
	end)

	local tooltipReplace : string = nil
	if ratio == 1 then
		tooltipReplace = _("Replace All Vehicles")
	else
		tooltipReplace = nGetText("Replace Selected Vehicle", "Replace Selected Vehicles", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconToggleButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abReplace,
		function(val : boolean)
			if val then
				log.verbose("Replace vehicles")

				local vehicleEntities : {Engine.Entity} = {}
				for __, vehicleEAR in ipairs(vehicleEARSelection) do
					if not entity_util.entityChanged0(vehicleEAR) then
						table.insert(vehicleEntities, vehicleEAR.entity)
					end
				end

				local wc = params.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)

				local transportModesAndCarrier = line_util.getLineAndVehicleCompatibleTransportModesAndCarrier(vehicleEntities)

				params.commonParams.vehicleStoreOpenForDepotAndLineRef:set({
					depot = nil,
					line = nil,
					vehicleStoreMode = "Replace",
				})
				params.vehicleStoreEventParamState:set({
					vehicleEntities = vehicleEntities,
					carrier = transportModesAndCarrier.carrier,
					transportModes = transportModesAndCarrier.transportModes,
					filterTags = vehicle_store_util.getVehicleFilterTags(vehicleEntities),
					getFirstStopToSendTo = params.commonParams.getFirstStopToSendTo,

					onClose = function()
						params.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)
					end,
				})
			else
				params.commonParams.vehicleManagerRef:get():getApi().closeVehicleStore()
			end
		end,
		tooltipReplace,
		selectionReplacableState:old(),
		nil,
		moreOptionsLocalState:old(),
		selectionReplacableState:old() and params.commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil
			and params.commonParams.vehicleStoreOpenForDepotAndLineRef:get().vehicleStoreMode == "Replace"
	))

	local selectionModifiableState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		return allSelectedVehiclesInSingleCarrier(true, true)
	end)

	local tooltipModify : string = nil
	if ratio == 1 then
		tooltipModify = _("Modify All Vehicles")
	else
		tooltipModify = nGetText("Modify Selected Vehicle", "Modify Selected Vehicles", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconToggleButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abConfigure,
		function(val : boolean)
			if val then
				log.verbose("Modify vehicles")

				local vehicleEntities : {Engine.Entity} = {}
				for __, vehicleEAR in ipairs(vehicleEARSelection) do
					if not entity_util.entityChanged0(vehicleEAR) then
						table.insert(vehicleEntities, vehicleEAR.entity)
					end
				end

				local wc = params.commonParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)

				params.vehicleStoreEventParamState:set({
					vehicleEntities = vehicleEntities,
					carrier = mirroredVehicleManagerState:old().carriers[1],
					transportModes = nil,
					filterTags = vehicle_store_util.getVehicleFilterTags(vehicleEntities),
					getFirstStopToSendTo = params.commonParams.getFirstStopToSendTo,

					onClose = function()
						params.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)
					end,
				})
				params.commonParams.vehicleStoreOpenForDepotAndLineRef:set({
					depot = nil,
					line = nil,
					vehicleStoreMode = "Modify",
				})
			else
				params.commonParams.vehicleManagerRef:get():getApi().closeVehicleStore()
			end
		end,
		tooltipModify,
		selectionModifiableState:old(),
		nil,
		moreOptionsLocalState:old(),
		selectionReplacableState:old() and params.commonParams.vehicleStoreOpenForDepotAndLineRef:get() ~= nil
			and params.commonParams.vehicleStoreOpenForDepotAndLineRef:get().vehicleStoreMode == "Modify"
	))

	table.insert(actionListChildren, builtin.Component{
		meta = {
			enabled = false,
			disableFocusableRecursive = true,
			class = "horizontal-spacer",
		}
	})

	local tooltipSendToDepot : string = nil
	if ratio == 1 then
		tooltipSendToDepot = _("Send All Vehicles to Depot")
	else
		tooltipSendToDepot = nGetText("Send Selected Vehicle to Depot", "Send Selected Vehicles to Depot", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconButton(
		"vehicle-button, secondary, send-to-depot-sfx",
		params.commonParams.iconPaths.abSendToDepot,
		function()
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				api.cmd.sendCommand(api.cmd.makeVehicleSendToDepotCmd(vehicleEAR.entity, false),
					function(_result : ICommandData, success : boolean)
					log.verbose("Command send to depot vehicle" .. tostring(vehicleEAR.entity) .. ": " .. (success and "success" or "fail"))
					if not success then
						params.commonParams.addFeedback(_("Vehicle could not be sent to depot."), nil, nil, nil)
					end
				end)
			end
		end,
		tooltipSendToDepot,
		#vehicleEARSelection > 0,
		"menu.management.sendToDepot",
		moreOptionsLocalState:old()
	))

	local tooltipSellVehicles : string = nil
	if ratio == 1 then
		tooltipSellVehicles = _("Sell All Vehicles")
	else
		tooltipSellVehicles = nGetText("Sell Selected Vehicle", "Sell Selected Vehicles", countSelected)
	end

	table.insert(actionListChildren, button_react_util.makeIconButton(
		"vehicle-button, secondary",
		params.commonParams.iconPaths.abSell,
		function()
			for __, vehicleEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				if params.commonParams.gameCtx.filters:get().protectedEntities[vehicleEAR.entity] then
					params.commonParams.addFeedback(_("Vehicle cannot be sold at this time."), nil, nil, nil)
					return
				end
			end

			params.commonParams.addFeedback(tooltipSellVehicles, "Question", {
				onAccept = function()
					local vehicleEntities : {Engine.Entity} = {}
					for __, vehicleEAR in ipairs(vehicleEARSelection) do
						if not entity_util.entityChanged0(vehicleEAR) then
							table.insert(vehicleEntities, vehicleEAR.entity)
						end
					end

					if #vehicleEntities > 0 then
						api.cmd.sendCommand(api.cmd.makeVehicleSellCmd(vehicleEntities))
						params.vehicleManagerApi.removeVehiclesFromVehicleList(vehicleEntities)
					end
				end,
				acceptText = tooltipSellVehicles,
				soundOnAccept = "SellVehicle",
			}, 2)
		end,
		tooltipSellVehicles,
		#vehicleEARSelection > 0,
		nil,
		moreOptionsLocalState:old()
	))

	table.insert(actionListChildren, builtin.Component{
		meta = { class = "horizontal-fixed-size-spacer" }
	})

	table.insert(actionListChildren, SendToLineButton{
		commonParams = params.commonParams,

		vehicleEARSelection = vehicleEARSelection,

		firstStopToSendSchemesRef = params.firstStopToSendSchemesRef,
		firstStopToSendSchemeMapRef = params.firstStopToSendSchemeMapRef,

		sendToLineDisabledState = params.sendToLineDisabledState,
		focusedLastState = focusedLastState,

		afterSendingToLineFixup = params.afterSendingToLineFixup,
	})

	return builtin.FloatingLayout{
		children = {
			builtin.FloatingLayoutChild{
				h = -1,
				v = -1,
				item = builtin.Component{
					layout = builtin.BoxLayout{
						orientation = builtin.type.Orientation.Horizontal,
						children = actionListChildren,
					},
				},
			},
			builtin.FloatingLayoutChild{
				h = 0,
				v = 0,
				item = builtin.Component(react.ref(params.commonParams.sendToLinePopoverPosRef), {
					mouseTransparent = true,
					meta = {
						class = "popover-anchor",
					},
				}),
			},
		},
	}
end)

local VehicleManager = react.RegisterRecipe("VehicleManager", function(params : VehicleManagerParams) : TreeNodeId
	local selfRef = react.useSelfRef() as ReactNodeRef

	local mirroredVehicleManagerState = react.useMirrorState(params.commonParams.vehicleManagerStateRef)

	react.onEvent("cannotAssignToLine", function(__ : string, failureParam : Failure)
		if failureParam ~= nil then
			local msg = _("Vehicles could not be assigned to line.")
			if failureParam.reason == api.type.Failure.Reason.NoElectric then
				msg = msg .. "\n" ..  _("Missing electric tracks.")
			elseif failureParam.reason == api.type.Failure.Reason.NoTram then
				msg = msg .. "\n" .. _("Missing tram tracks.")
			elseif failureParam.reason == api.type.Failure.Reason.NoTramButTramTrack then
				msg = msg .. "\n" .. _("Incompatible tracks.")
			elseif failureParam.reason == api.type.Failure.Reason.NoTramTrackButRail then
				msg = msg .. "\n" .. _("Missing tram support on tracks.") -- via mod only, vanilla doesn't have such tracks
			elseif failureParam.reason == api.type.Failure.Reason.NoSmallAircraft then
				msg = msg .. "\n" .. _("No path for big airplanes.")
			elseif failureParam.reason == api.type.Failure.Reason.NoSmallShip then
				msg = msg .. "\n" .. _("No path for big ships.")
			end
			params.commonParams.addFeedback(msg, nil, nil, nil)
		else
			params.commonParams.addFeedback(_("Vehicles could not be assigned to line."), nil, nil, nil)
		end
	end)

	react.onStep(function()
		local selected : {EntityUtil.EntityAndRevision} = {}
		local unselected : {EntityUtil.EntityAndRevision} = {}
		local changed = false

		for __, vehicleEAR in ipairs(mirroredVehicleManagerState:old().vehicleListEntitiesSelected) do
			if entity_util.entityChanged0(vehicleEAR) then
				changed = true
			else
				table.insert(selected, vehicleEAR)
			end
		end
		for __, vehicleEAR in ipairs(mirroredVehicleManagerState:old().vehicleListEntitiesUnselected) do
			if entity_util.entityChanged0(vehicleEAR) then
				changed = true
			else
				table.insert(unselected, vehicleEAR)
			end
		end

		if changed then
			params.commonParams.vehicleManagerStateRef:set(
				makeVehicleManagerState(selected, unselected)
			)
		end
	end)

	local vehicleManagerApi : VehicleManagerApi = {
		addVehiclesToVehicleListAndSelect = function(vehicleEntities : {Engine.Entity})
			local selected = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected
			local unselected = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected

			for __, entityToAdd in ipairs(vehicleEntities) do
				local found = false
				for __, vEAR in ipairs(unselected) do
					if (entityToAdd == vEAR.entity) then
						found = true
					end
				end
				if not found then
					for __, vEAR in ipairs(selected) do
						if (entityToAdd == vEAR.entity) then
							found = true
						end
					end
				end
				if not found then
					table.insert(selected, entity_util.makeEntityAndRevision(entityToAdd))
				end
			end

			params.commonParams.vehicleManagerStateRef:set(makeVehicleManagerState(selected, unselected))
		end,
		removeVehiclesFromVehicleList = function(vehicleEntities : {Engine.Entity})
			local selected : {EntityUtil.EntityAndRevision} = {}
			local unselected : {EntityUtil.EntityAndRevision} = {}
			local selectedOld = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected
			local unselectedOld = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected

			for __, vEAR in ipairs(selectedOld) do
				local removeEntity = false
				for __, entityToRemove in ipairs(vehicleEntities) do
					if (entityToRemove == vEAR.entity) then
						removeEntity = true
					end
				end
				if not removeEntity then
					table.insert(selected, vEAR)
				end
			end

			for __, vEAR in ipairs(unselectedOld) do
				local removeEntity = false
				for __, entityToRemove in ipairs(vehicleEntities) do
					if (entityToRemove == vEAR.entity) then
						removeEntity = true
					end
				end
				if not removeEntity then
					table.insert(unselected, vEAR)
				end
			end

			params.commonParams.vehicleManagerStateRef:set(makeVehicleManagerState(selected, unselected))
		end,
		isVehicleInVehicleList = function(vehicleEntity : Engine.Entity) : boolean
			for __, vEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				if vEAR.entity == vehicleEntity then
					return true
				end
			end
			for __, vEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected) do
				if vEAR.entity == vehicleEntity then
					return true
				end
			end
			return false
		end,
		areAnyVehiclesInVehicleList = function() : boolean
			return (
				#params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected > 0
				or #params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected > 0
			)
		end,
		selectVehicles = function(vehicleEntities : {Engine.Entity}, selectEntities : boolean)
			for __, vehicleEntity in ipairs(vehicleEntities) do
				local selected : {EntityUtil.EntityAndRevision} = {}
				local unselected : {EntityUtil.EntityAndRevision} = {}
				local selectedOld = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected
				local unselectedOld = params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected

				if selectEntities then
					for __, vEAR in ipairs(unselectedOld) do
						if (vehicleEntity == vEAR.entity) then
							table.insert(selected, vEAR)
						else
							table.insert(unselected, vEAR)
						end
					end

					for __, selectedVehicle in ipairs(selectedOld) do
						table.insert(selected, selectedVehicle)
					end
				else
					for __, vEAR in ipairs(selectedOld) do
						if (vehicleEntity == vEAR.entity) then
							table.insert(unselected, vEAR)
						else
							table.insert(selected, vEAR)
						end
					end

					for __, unselectedVehicle in ipairs(unselectedOld) do
						table.insert(unselected, unselectedVehicle)
					end
				end

				params.commonParams.vehicleManagerStateRef:set(makeVehicleManagerState(selected, unselected))
			end
		end,
		isVehicleSelected = function(vehicleEntity : Engine.Entity) : boolean
			for __, vEAR in ipairs(params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected) do
				if vEAR.entity == vehicleEntity then
					return true
				end
			end
			return false
		end,
		numberSelectedVehicles = function() : integer
			return #params.commonParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected
		end,
		sendVehiclesToLine = function(
			vehicleSelection : {Engine.Entity},
			lineEntity : Engine.Entity,
			sendScheme : FirstStopToSend.Scheme,
			callback : function(vehicleSelection : {Engine.Entity})
		)
			sendVehiclesToLineCommands(
				vehicleSelection,
				lineEntity,
				sendScheme,
				callback,
				params.commonParams.bufferedSendDataRef,
				false -- doNotBuffer
			)
		end,
		sendVehiclesToDepot = function(vehicleSelection : {Engine.Entity}, __depotEntity : Engine.Entity)
			for __, vehicleEntity in ipairs(vehicleSelection) do
				api.cmd.sendCommand(api.cmd.makeVehicleSendToDepotCmd(vehicleEntity, false),
						function(_result : ICommandData, success : boolean)
					log.verbose("Command send to depot vehicle" .. tostring(vehicleEntity) .. ": " .. (success and "success" or "fail"))
					if not success then
						params.commonParams.addFeedback(_("Vehicle could not be sent to depot."), nil, nil, nil)
					end
				end)
			end
		end,
		clearVehicleList = function()
			params.commonParams.vehicleManagerStateRef:set(makeVehicleManagerState({}, {}))
		end,
		closeVehicleStore = function()
			local wc = params.commonParams.gameCtx.windowContainer:get():getApi()
			wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
			params.commonParams.vehicleStoreOpenForDepotAndLineRef:set(nil)
		end,
	}
	react.provideApi(vehicleManagerApi)

	local vehicleManagerActionButtons = VehicleManagerActionButtons(react.ref(params.vehicleManagerActionButtonsRef), {
		meta = {
			onFocusChange = function(focused)
				params.commonParams.lvmActionButtonsState:set(focused and "VMAB" or "NONE")
			end,
		},

		firstStopToSendSchemesRef = params.firstStopToSendSchemesRef,
		firstStopToSendSchemeMapRef = params.firstStopToSendSchemeMapRef,

		sendToLineDisabledState = params.sendToLineDisabledState,
		vehicleStoreEventParamState = params.vehicleStoreEventParamState,

		commonParams = params.commonParams,

		vehicleManagerApi = vehicleManagerApi,

		afterSendingToLineFixup = params.afterSendingToLineFixup,
	})

	local oldVehicles = {}
	if mirroredVehicleManagerState:old() then
		for __, selectedVehicleEAR in ipairs(mirroredVehicleManagerState:old().vehicleListEntitiesSelected) do
			if not entity_util.entityChanged0(selectedVehicleEAR) then
				if entity_util.isOwnedByPlayer(selectedVehicleEAR.entity) then
					table.insert(oldVehicles, selectedVehicleEAR.entity)
				end
			end
		end
		for __, unselectedVehicleEAR in ipairs(mirroredVehicleManagerState:old().vehicleListEntitiesUnselected) do
			if not entity_util.entityChanged0(unselectedVehicleEAR) then
				if entity_util.isOwnedByPlayer(unselectedVehicleEAR.entity) then
					table.insert(oldVehicles, unselectedVehicleEAR.entity)
				end
			end
		end
	end

	params.vehicleListRef:set(nil)

	local vehicleManager = builtin.Component{
		meta = {
			tag = "menu.management.vehicle",
		},
		layout = builtin.BoxLayout{
			children = #oldVehicles > 0 and {
				vehicle_list_react_util.VehicleList(react.ref(params.vehicleListRef), {
					vehicles = oldVehicles,

					managerRef = selfRef,
					managerActionButtonsRef = params.vehicleManagerActionButtonsRef,

					commonParams = params.commonParams,

					onClickSelectLineOrDepot = function(entity : Engine.Entity, isLine : boolean)
						if isLine then
							local ladlApi : LineAndDepotListApi = nil
							if params.commonParams.lineAndDepotListRef:get() then
								ladlApi = params.commonParams.lineAndDepotListRef:get():getApi()
							end

							if vehicleManagerApi and ladlApi then
								selectSingleLineAndOverrideSelectionOnClick(
									vehicleManagerApi,
									ladlApi,
									params.commonParams.creatingNewLineState,
									entity,
									false,
									nil
								)
							end
							params.scrollToRow(entity, true)
						else -- depot
							params.scrollToRow(entity, true)
						end
					end,
					onClickSelectVehicle = function(entity : Engine.Entity)
						vehicleManagerApi.selectVehicles(
							{entity},
							not vehicleManagerApi.isVehicleSelected(entity)
						)
					end,
				})
			} or nil,
		},
	}

	local noVehiclesInListNotice = (mirroredVehicleManagerState:old() and (
		(#mirroredVehicleManagerState:old().vehicleListEntitiesSelected == 0)
		and (#mirroredVehicleManagerState:old().vehicleListEntitiesUnselected == 0)
	)) and builtin.Component{
		layout = builtin.BoxLayout{
			orientation = builtin.type.Orientation.Horizontal,
			children = {
				gui_react_util.makeHorizontalSpacer(),
				builtin.TextView{
					meta = {
						class = "empty-text, font-scale-body" ..
							(params.commonParams.creatingNewLineState:old() and ", do-not-show" or ""),
					},
					text = (params.commonParams.gamepadInputModeState:old() and
						((params.commonParams.lvm2D3DModeState:old() == "2D") and _("No vehicle, depot or line selected.") or "")) or
						_("Select a vehicle, depot or line."),
				},
				gui_react_util.makeHorizontalSpacer(),
			},
		},
	} or nil

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Vertical,
		children = {
			vehicleManagerActionButtons,
			vehicleManager,
			noVehiclesInListNotice,
		},
	}
end) as RecipeWithApi<VehicleManagerParams, VehicleManagerApi>

local record LineVehicleManagerTopBarParams
	commonParams : CommonActionParams

	filterState : ReactStateT<StatisticsReactUtil.FilterState>
end

local LineVehicleManagerTopBar = react.RegisterRecipe("LineVehicleManagerTopBar", function(params : LineVehicleManagerTopBarParams) : TreeNodeId
	local dummyRef = react.useNodeRef()
	react.onMount(function()
		if dummyRef:get() then
			dummyRef:get():focus()
		end
	end)

	local makeLvmModeDisplay = function(lvmMode : LVMMode, prev : boolean) : TreeNodeId
		local modeText : string = nil
		local modeTooltip : string = nil
		if lvmMode == "MAIN" then
			modeText = "M"
			modeTooltip = _("Main Management Mode")
		elseif lvmMode == "SEND_TO_LINE" then
			modeText = "SL"
			modeTooltip = _("Send to Line Mode")
		elseif lvmMode == "SEGMENT" then
			modeText = "SEG"
			modeTooltip = _("Line Segment Mode")
		elseif lvmMode == "MOVE_STOP" then
			modeText = "MS"
			modeTooltip = _("Move Stop Mode")
		elseif lvmMode == "MOVE_WAYPOINT" then
			modeText = "MW"
			modeTooltip = _("Move Waypoint Mode")
		end
		if prev then
			modeTooltip = _("Previous") .. " - " .. modeTooltip
		end
		return builtin.TextView{
			meta = {
				class = "mode-icon, font-scale-annotation, " .. (prev and "mode-previous" or ""),
				tooltip = modeTooltip,
			},
			text = modeText,
		}
	end

	local showLvmMode = false -- NOTE: optional display for current LVM mode
	local lvmModeDisplay : TreeNodeId = nil
	if showLvmMode then
		local lvmMode = params.commonParams.getModeState()
		lvmModeDisplay = makeLvmModeDisplay(lvmMode[1], false)
	end

	local showPrevLvmMode = false -- NOTE: optional display for previous LVM mode
	local prevLvmModeDisplay : TreeNodeId = nil
	if showPrevLvmMode then
		local prevLvmMode = params.commonParams.prevLvmModeRef:get()
		if prevLvmMode then
			prevLvmModeDisplay = makeLvmModeDisplay(prevLvmMode[1], true)
		end
	end

	local showBufferedData = false -- NOTE: optional display for buffered send to line data
	local lvmBufferedDataDisplay : TreeNodeId = nil
	if showBufferedData then
		local bufferedCount = 0
		local bufferedCountTooltip : string = ""
		local bufferedData = params.commonParams.bufferedSendDataRef:get()
		if bufferedData then
			bufferedCount = #bufferedData
			for i, data in ipairs(bufferedData) do
				if api.engine.entityExists(data.line) and api.engine.entityExists(data.vehicle) then
					local vehicleName = api.engine.util.getEntityName(data.vehicle) or ""
					local lineName = api.engine.util.getEntityName(data.line) or ""
					local stopIndex = data.stopToSend
					local line : string = vehicleName .. " -> " .. lineName .. " @ " .. lang_util.formatInt(stopIndex)
					if i < bufferedCount then
						line = line .. "\n"
					end
					bufferedCountTooltip = bufferedCountTooltip .. line
				end
			end
		end
		local bufferedCountText = lang_util.formatInt(bufferedCount)

		lvmBufferedDataDisplay = builtin.TextView{
			meta = {
				class = "mode-icon, font-scale-annotation",
				tooltip = bufferedCountTooltip,
			},
			text = bufferedCountText,
		}
	end

	local topBarChildren = {
		builtin.Component{
			meta = {
				class = "vehicle-action-list",
			},
			layout = builtin.BoxLayout{
				orientation = builtin.type.Orientation.Horizontal,
				children = {
					not params.commonParams.gamepadInputModeState:old() and builtin.Component(react.ref(dummyRef), {
						meta = {
							forceFocusable = true,
						},
					}) or nil,
					not params.commonParams.gamepadInputModeState:old() and statistics_react_util.createSearchField(
						params.filterState:old().search,
						function(newValue : string) : string
							params.filterState:transform(function(cur : StatisticsReactUtil.FilterState) : StatisticsReactUtil.FilterState
								local copy = table_util.copy(cur)
								copy.search = newValue
								return copy
							end)
						end,
						nil
					) or nil,
					gui_react_util.makeHorizontalSpacer(),
					lvmBufferedDataDisplay,
					prevLvmModeDisplay,
					lvmModeDisplay,
					-- builtin.ToggleButton{
					-- 	meta = {
					-- 		class = "title-bar-button",
					-- 		tooltip = _("Simplified View")
					-- 	},
					-- 	content = builtin.ImageView{
					-- 		path = "::/gui/line_vehicle_mgmt/icons/line_stations.tga"
					-- 	},
					-- 	onValueChange = function(i : integer)
					-- 		params.commonParams.gameCtx.metroMode:set(i == 1)
					-- 	end,
					-- 	value = params.commonParams.gameCtx.metroMode:old() and 1 or 0
					-- },
					-- builtin.ToggleButton{
					-- 	meta = {
					-- 		class = "title-bar-button",
					-- 		tooltip = _("Color Mode")
					-- 	},
					-- 	content = builtin.ImageView{
					-- 		path = "::/gui/line_vehicle_mgmt/icons/terrain_mode.tga"
					-- 	},
					-- 	onValueChange = function(i : integer)
					-- 		params.commonParams.gameCtx.colorMode:set(i == 1)
					-- 	end,
					-- 	value = params.commonParams.gameCtx.colorMode:old() and 1 or 0
					-- },
				},
			},
		},
	}

	return builtin.BoxLayout{
		orientation = builtin.type.Orientation.Horizontal,
		children = topBarChildren,
	}
end)

local manager_window : ManagerWindow = {}

manager_window.ManagerWindowContent = react.RegisterRecipe("ManagerWindowContent",
		function(params : ToolReactUtil.ToolWindowParamPayload<ManagerWindow.ManagerToolParam>) : TreeNodeId
	local toolParams = params.toolParam
	local gameCtx = toolParams.gameCtx

	local selfRef = react.useSelfRef() as ReactNodeRef

	local feedbackViewerRef = react.useNodeRef(feedback_list_util.FeedbackList)

	local lvm2D3DModeState : ReactStateT<LVM2D3DMode> = react.useState("2D")
	local lvmSectionState : ReactStateT<LVMSection> = react.useState("LM")
	local lvmActionButtonsState : ReactStateT<LVMActionButtons> = react.useState("NONE")
	local gamepadInputModeState : ReactStateT<boolean> = engine_react_util.useStepStateTimer(function() : boolean
		return api.util.getInputMode() == api.type["enum"].InputMode.Gamepad
	end)

	local lvmModeState : ReactStateT<{LVMMode, any}> = react.useState({"MAIN", nil})
	local moreOptionsHoldState : ReactStateT<boolean> = react.useState(false)
	local creatingNewLineState : ReactStateT<boolean> = react.useState(false)
	local gamepadNo3DSelectionInfoState : ReactStateT<boolean> = react.useState(false)
	local blockNewLineState : ReactStateT<boolean> = react.useState(false)
	-- TPF3-MP: the stations clicked while a new line is being created. In a
	-- multiplayer room the line exists only once the room has created it in
	-- every game, a moment after the first click, and until then a click on
	-- a station would start a new line again, which is blocked, and the stop
	-- would be lost. They are kept here and added to the line once it is
	-- loaded (queuedStopsLineRef: the line they wait for).
	local queuedStopsRef : ReactRefT<{LineUtil.StationSelectionDetails}> = react.useRef({})
	local queuedStopsLineRef : ReactRefT<Engine.Entity> = react.useRef(nil)
	local sendToLineHoveredLineState : ReactStateT<Engine.Entity> = react.useState(nil)
	local addStopHoveredStationState : ReactStateT<LineUtil.StationSelectionDetails> = react.useState(nil)
	local preferLineOverTNEdgeState : ReactStateT<boolean> = react.useState(true)

	local vehicleStoreEventParamState : ReactStateT<VehicleStoreWindow.VehicleStoreEventParam> = react.useState(nil)
	local vehicleStoreOpenForDepotAndLineRef : ReactRefT<VehicleStoreOpenInfo> = react.useRef()
	local sendToLineDisabledState : ReactStateT<boolean> = react.useState(false)
	local openedWithSendToLineRef : ReactRefT<boolean> = react.useRef(false)
	local hasFeedbackState : ReactStateT<boolean> = react.useState(false)
	local scrollToNewLineRef : ReactRefT<Engine.Entity> = react.useRef(nil)
	local deleteSellRef : ReactRefT<LVMDeleteSell> = react.useRef({
		focusedLine = nil,
		focusedVehicle = nil,

		focusedStopLine = nil,
		focusedStopNumber = nil,
		focusedApiStopIndex0 = nil,
	})
	local bufferedSendDataRef : ReactRefT<{BufferedLVMSendData}> = react.useRef({})
	local prevLvmModeRef : ReactRefT<{LVMMode, any}> = react.useRef(nil)
	local depot2VehiclesCompareRef : ReactRefT<{Engine.Entity : {Engine.Entity}}> = react.useRef({})
	local lvmDnDDataRef : ReactRefT<LVMDnDData> = react.useRef({})
	local lvmInitialControllerFocusRef = react.useRef(true)
	local lvmExit3DControllerFocusRef : ReactRefT<integer> = react.useRef(2)

	local configureLinesPopoverPosRef = react.useNodeRef()
	local sendToLinePopoverPosRef = react.useNodeRef()
	local btnSendToLineRef = react.useNodeRef()
	local btnBuyVehiclesRef = react.useNodeRef()
	local btnNewLineRef = react.useNodeRef()

	local vehicleManagerActionButtonsRef : ReactRefWrap = react.useNodeRef(VehicleManagerActionButtons)
	local vehicleListRef : ReactRefWrap = react.useNodeRef(vehicle_list_react_util.VehicleList)
	local vehicleManagerRef = react.useNodeRef(VehicleManager)
	local lineManagerActionButtonsRef = react.useNodeRef(LineManagerActionButtons)
	local lineAndDepotListRef = react.useNodeRef(LineAndDepotList)
	local linePanelActionButtonsRef = react.useNodeRef(LinePanelActionButtons)
	local linePanelRef = react.useNodeRef(LineManagerPanel)

	local cacheUpdateCounterSysRef : ReactRefT<integer> = react.useRef(0)
	local cacheUpdateCounterLineRef : ReactRefT<integer> = react.useRef(0)
	local cacheUpdatesPerTick = 4

	local problemCacheState : ReactStateT<ProblemCache> = engine_react_util.useStepState(function(curr : ProblemCache) : ProblemCache
		if not curr then
			return {
				vehiclesAndProblems = {},
				vehiclesWithRoundTrips = {},
				line2misconfiguredLines = {},
				line2detailedLineProblems = {},
				line2problem = {},
				vehicle2problem = {},
				line2vehicleProblemCount = {},
			}
		end

		local lineEARsSelected = toolParams.lineManagerStateRef:get().lineListEntitiesSelected
		local lineEARsUnselected = toolParams.lineManagerStateRef:get().lineListEntitiesUnselected
		local lineCount = #lineEARsSelected + #lineEARsUnselected
		for __i = 1, cacheUpdatesPerTick do
			if cacheUpdateCounterLineRef:get() > lineCount then
				cacheUpdateCounterLineRef:set(0)
				break
			end

			local lineEAR : EntityUtil.EntityAndRevision
			local index = cacheUpdateCounterLineRef:get()
			if index > 0 then
				if index <= #lineEARsSelected then
					lineEAR = lineEARsSelected[index]
				elseif index <= #lineEARsSelected + #lineEARsUnselected then
					lineEAR = lineEARsUnselected[index - #lineEARsSelected]
				end
			end

			if lineEAR and not entity_util.entityChanged0(lineEAR) then
				local detailedLineProblems = api.engine.util.line.getDetailedLineProblems(lineEAR.entity)
				curr.line2detailedLineProblems[lineEAR.entity] = detailedLineProblems
				local lineStopProblemsCount = 0
				for __, segment in ipairs(detailedLineProblems) do
					for __, stop in ipairs(segment) do
						if stop.noPath == true
							or stop.noPathToAlternative ~= nil
							or stop.noPathFromAlternative ~= nil
							or stop.duplicateStop == true
							or stop.incompatibleStop == true then

							lineStopProblemsCount = lineStopProblemsCount + 1
						end
					end
				end
				local lineHasProblems = false
				local lineIssue = curr.line2misconfiguredLines[lineEAR.entity]
				if lineStopProblemsCount > 0 or lineIssue ~= nil then
					lineHasProblems = true
				end
				curr.line2problem[lineEAR.entity] = lineHasProblems

				local problemCount = 0
				local lineVehicles = api.engine.system.transportVehicleSystem.getLineVehicles(lineEAR.entity)
				for __, lineVehicle in ipairs(lineVehicles) do
					local problem = false

					-- vehicle problems
					if not problem then
						for __, vehiclesAndProblem in ipairs(curr.vehiclesAndProblems) do
							for __, vehicleEntity in ipairs(vehiclesAndProblem[1]) do
								if lineVehicle == vehicleEntity then
									problem = true
									break
								end
							end
						end
					end
					-- vehicle unload problems (cargo round trip)
					if not problem then
						problem = curr.vehiclesWithRoundTrips[lineVehicle] ~= nil
					end

					if problem then
						problemCount = problemCount + 1
					end
					curr.vehicle2problem[lineVehicle] = problem
				end

				curr.line2vehicleProblemCount[lineEAR.entity] = problemCount
			end

			cacheUpdateCounterLineRef:set(cacheUpdateCounterLineRef:get() + 1)
		end

		-- whole system update calls
		if cacheUpdateCounterSysRef:get() == 0 then
			curr.vehiclesAndProblems = api.engine.util.vehicle.getVehicleProblems()
		end
		if cacheUpdateCounterSysRef:get() == 1 then
			curr.vehiclesWithRoundTrips = api.engine.system.simEntityAtVehicleSystem.getVehiclesWithRoundTripCount()
		end
		if cacheUpdateCounterSysRef:get() == 2 then
			curr.line2misconfiguredLines = api.engine.util.line.getLinesIssues(api.engine.util.getPlayer(), false)
		end
		cacheUpdateCounterSysRef:set((cacheUpdateCounterSysRef:get() + 1) % 3)

		return curr
	end)

	local function getLVMMode() : {LVMMode, any}
		return lvmModeState:old()
	end

	-- do not set lvmModeState directly, always go through this function
	local function switchLVMMode(modeAndParams : {LVMMode, any})
		if modeAndParams == nil then
			toolParams.onBack()
		else
			local oldMode = lvmModeState:old()
			if oldMode then
				if oldMode[1] == "SEGMENT" or oldMode[1] == "MAIN" then
					prevLvmModeRef:set(oldMode)
				end
			end
			lvmModeState:set(modeAndParams)
			toolParams.storedLVMModeRef:set(modeAndParams)
		end
	end
	local function switchToPrevLVMMode()
		local prevMode = prevLvmModeRef:get()
		if prevMode ~= nil then
			if prevMode[1] == "SEGMENT" then
				switchLVMMode(prevLvmModeRef:get())
			else
				switchLVMMode({"MAIN", nil})
			end
		end
	end

	local afterSendingToLineFixup = function(sentEntities : {Engine.Entity})
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end
		if vmApi then
			local selectedLineEARs = toolParams.lineManagerStateRef:get().lineListEntitiesSelected
			for __, sentEntity in ipairs(sentEntities) do
				local tv = api.engine.getComponent(sentEntity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
				local lineSelected = false
				for __, selectedLineEAR in ipairs(selectedLineEARs) do
					if tv.line == selectedLineEAR.entity then
						lineSelected = true
						break
					end
				end
				if not lineSelected then
					vmApi.removeVehiclesFromVehicleList({sentEntity})
				end
			end
		end
	end

	local lvmFilterConfigState : ReactStateT<LVMFilterConfig> = react.useState(nil)
	local localLVMFilterState : ReactStateT<StatisticsReactUtil.FilterState> = react.useState(toolParams.lvmFilterStateRef:get())
	toolParams.lvmFilterStateRef:set(localLVMFilterState:old())

	local carrierFn = function() : {Carrier}
		if not lvmFilterConfigState:old() then
			return nil
		end
		if not localLVMFilterState:old() then
			return nil
		end
		if lvmFilterConfigState:hasExpired() then
			return nil
		end
		if localLVMFilterState:hasExpired() then
			return nil
		end
		return statistics_react_util.getCarrierFilterFromCategories(lvmFilterConfigState:old().categories, localLVMFilterState:old().category)
	end

	local countDataState : ReactStateT<LVMCountData> = engine_react_util.useStepState(function() : LVMCountData
		local countLinesWithProblems = 0
		local countLinesSelected = 0
		local countLinesUnselected = 0
		local selectionRatioLines : number = 0

		local countDepotsSelected = 0
		local countDepotsUnselected = 0
		local selectionRatioDepots : number = 0

		local countVehiclesWithProblems = 0
		local countVehiclesSelected = #toolParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected
		local countVehiclesUnselected = #toolParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected
		local selectionRatioVehicles : number = 0

		local countVehiclesTotal = countVehiclesSelected + countVehiclesUnselected
		if countVehiclesTotal > 0 then
			selectionRatioVehicles = countVehiclesSelected / countVehiclesTotal
		end

		local onlyVisible = localLVMFilterState:old().onlyVisible
		local allowedCarriers : {Carrier} = carrierFn()
		local problemCache = problemCacheState:old() or {
			vehiclesAndProblems : {{{Engine.Entity}, VehicleProblem}} = {},
			vehiclesWithRoundTrips : {Engine.Entity : integer} = {},
			line2misconfiguredLines : {Engine.Entity : {LineIssue}} = {},
			line2detailedLineProblems : {Engine.Entity : {{UtilLine.StopState}}} = {},
			line2problem : {Engine.Entity : boolean} = {},
			vehicle2problem : {Engine.Entity : boolean} = {},
			line2vehicleProblemCount : {Engine.Entity : integer} = {},
		}

		for __, depotEAR in ipairs(toolParams.lineManagerStateRef:get().depotListEntitiesSelected) do
			local depot : Engine.Component.VehicleDepot = api.engine.getComponent(depotEAR.entity, api.type.ComponentType.VEHICLE_DEPOT)

			if not entity_util.entityChanged0(depotEAR) then
				if not onlyVisible or api.gui.byEntity.isVisible(depotEAR.entity) then
					if not allowedCarriers or statistics_react_util.areCarriersContainedInTransportModes(depot.transportModes, allowedCarriers) then
						countDepotsSelected = countDepotsSelected + 1
					end
				end
			end
		end

		for __, depotEAR in ipairs(toolParams.lineManagerStateRef:get().depotListEntitiesUnselected) do
			local depot : Engine.Component.VehicleDepot = api.engine.getComponent(depotEAR.entity, api.type.ComponentType.VEHICLE_DEPOT)

			if not entity_util.entityChanged0(depotEAR) then
				if not onlyVisible or api.gui.byEntity.isVisible(depotEAR.entity) then
					if not allowedCarriers or statistics_react_util.areCarriersContainedInTransportModes(depot.transportModes, allowedCarriers) then
						countDepotsUnselected = countDepotsUnselected + 1
					end
				end
			end
		end

		local countDepotsTotal = countDepotsSelected + countDepotsUnselected
		if countDepotsTotal > 0 then
			selectionRatioDepots = countDepotsSelected / countDepotsTotal
		end

		for __, lineEAR in ipairs(toolParams.lineManagerStateRef:get().lineListEntitiesSelected) do
			if not entity_util.entityChanged0(lineEAR) then
				if not onlyVisible or api.gui.byEntity.isLineEmptyOrVisible(lineEAR.entity) then
					if not allowedCarriers or api.engine.util.line.isLineCompatibleWithAnyCarrier(lineEAR.entity, allowedCarriers, false) then
						countLinesSelected = countLinesSelected + 1
					end
				end

				if problemCache.line2problem[lineEAR.entity] then
					countLinesWithProblems = countLinesWithProblems + 1
				end

				local lineVehicleProblemCount : integer = problemCache.line2vehicleProblemCount[lineEAR.entity] or 0
				countVehiclesWithProblems = countVehiclesWithProblems + lineVehicleProblemCount
			end
		end

		for __, lineEAR in ipairs(toolParams.lineManagerStateRef:get().lineListEntitiesUnselected) do
			if not entity_util.entityChanged0(lineEAR) then
				if not onlyVisible or api.gui.byEntity.isLineEmptyOrVisible(lineEAR.entity) then
					if not allowedCarriers or api.engine.util.line.isLineCompatibleWithAnyCarrier(lineEAR.entity, allowedCarriers, false) then
						countLinesUnselected = countLinesUnselected + 1
					end
				end

				if problemCache.line2problem[lineEAR.entity] then
					countLinesWithProblems = countLinesWithProblems + 1
				end
			end
		end

		local countLinesTotal = countLinesSelected + countLinesUnselected
		if countLinesTotal > 0 then
			selectionRatioLines = countLinesSelected / countLinesTotal
		end

		return {
			countLinesWithProblems = countLinesWithProblems,
			countLinesSelected = countLinesSelected,
			countLinesUnselected = countLinesUnselected,
			selectionRatioLines = selectionRatioLines,

			countDepotsSelected = countDepotsSelected,
			countDepotsUnselected = countDepotsUnselected,
			selectionRatioDepots = selectionRatioDepots,

			countVehiclesWithProblems = countVehiclesWithProblems,
			countVehiclesSelected = countVehiclesSelected,
			countVehiclesUnselected = countVehiclesUnselected,
			selectionRatioVehicles = selectionRatioVehicles,
		}
	end)

	local guiCtxForInputActions = react.useState(nil) as ReactStateT<LineUtil.GuiContextState>
	line_util.applyInputActionConfig(guiCtxForInputActions:old())

	-- can be lazy because the functions act like "pure functions" and do not depend on
	-- commonParams internal state
	local commonParams : CommonActionParams = react.useRefLazy(function() : CommonActionParams
		return {
			gameCtx = gameCtx,
			parent = selfRef,
			lvmButtonIAs = {
				btn2D3D_Switch      = "IA_OPTION1", -- face left

				btn2D_ActionButtons = "IA_OPTION2", -- face up
				btn3D_Move          = "IA_OPTION2", -- face up

				btn2D_MoreOptions   = "IA_OPTION3", -- left shoulder (hold)
				btn3D_Configure     = "IA_OPTION3", -- left shoulder

				btn2D_NextSection   = "IA_OPTION4", -- right shoulder
				btn3D_Delete        = "IA_OPTION4", -- right shoulder

				btn2D_MoveStopHold  = "IA_SELECT", -- face down (hold)

				btnMore_SendToLine  = "IA_OPTION1", -- face left
				btnMore_NewLine     = "IA_OPTION2", -- face up
				btnMore_DeleteSell  = "IA_MENU_BACK", -- face right
				btnMore_BuyVehicle  = "IA_SELECT", -- face down

				modifier3D_PreferWaypoint = "IA_PRECISION_MODE", -- right shoulder (hold)
			},
			iconPaths = { -- iconpaths for LVM section rows (smaller)
				rowRename          = "::/gui/line_vehicle_mgmt/icons/symbol_pencil_20.tga",
				rowConfigure       = "::/gui/line_vehicle_mgmt/icons/symbol_cargo_20.tga",
				rowMove            = "::/gui/line_vehicle_mgmt/icons/indicator_move_20.tga",
				rowDelete          = "::/gui/line_vehicle_mgmt/icons/symbol_trash_bin_20.tga",
				rowSelectTerminals = "::/gui/line_vehicle_mgmt/icons/indicator_terminal_20.tga",
				rowLocate          = "::/gui/line_vehicle_mgmt/icons/symbol_locate_20.tga",
				rowRemove          = "::/gui/line_vehicle_mgmt/icons/symbol_minus_20.tga",
				rowCart            = "::/gui/line_vehicle_mgmt/icons/symbol_shopping_cart_20.tga",

				problemAlert       = "::/gui/statistics/icons/alert.tga",
				problemArrow       = "::/gui/line_vehicle_mgmt/icons/special_arrow_down.tga",

				regSelectTerminals = "::/gui/line_vehicle_mgmt/icons/indicator_terminal.tga",
				regRename          = "::/gui/builtin/window/icons/symbol_pencil.tga",
				regAutoRename      = "::/gui/line_vehicle_mgmt/icons/symbol_autorename.tga",
				regFilterVisible   = "::/gui/line_vehicle_mgmt/icons/symbol_eye.tga",
				regGradient        = "::/gui/line_vehicle_mgmt/design/gradient.tga",
				regDragGrip        = "::/gui/line_vehicle_mgmt/icons/drag_vertical_20.tga",

				abNewLine          = "::/gui/line_vehicle_mgmt/icons/line_plus.tga",
				abSendToLine       = "::/gui/line_vehicle_mgmt/icons/symbol_arrow_up.tga",
				abBuyVehicles      = "::/gui/line_vehicle_mgmt/icons/symbol_shopping_cart.tga",
				abFilterVisible    = "::/gui/line_vehicle_mgmt/icons/symbol_eye.tga",
				abFilterRoad       = "::/gui/line_vehicle_mgmt/icons/vehicle_bus_18.tga",
				abFilterTram       = "::/gui/line_vehicle_mgmt/icons/vehicle_tram_18.tga",
				abFilterRail       = "::/gui/line_vehicle_mgmt/icons/vehicle_train_18.tga",
				abFilterWater      = "::/gui/line_vehicle_mgmt/icons/vehicle_ship_18.tga",
				abFilterAir        = "::/gui/line_vehicle_mgmt/icons/vehicle_airplane_18.tga",
				abReverse          = "::/gui/line_vehicle_mgmt/icons/symbol_arrow_reverse.tga",
				abStartStop        = "::/gui/line_vehicle_mgmt/icons/symbol_circle_on_off.tga",
				abPaint            = "::/gui/entity_window/icons/symbol_paintbucket.tga",
				abDuplicate        = "::/gui/line_vehicle_mgmt/icons/symbol_square_copy.tga",
				abReplace          = "::/gui/line_vehicle_mgmt/icons/symbol_arrows_refresh.tga",
				abConfigure        = "::/gui/line_vehicle_mgmt/icons/symbol_wrench.tga",
				abSendToDepot      = "::/gui/line_vehicle_mgmt/icons/building_depot_arrow.tga",
				abSell             = "::/gui/line_vehicle_mgmt/icons/symbol_dollar_arrow.tga",
				abAutoRename       = "::/gui/line_vehicle_mgmt/icons/symbol_autorename.tga",

				hudTerminal        = "::/gui/line_vehicle_mgmt/icons/indicator_terminal.tga",
				hudConfigureNone   = "::/gui/line_vehicle_mgmt/icons/configure.tga",
				hudConfigure       = "::/gui/line_vehicle_mgmt/icons/symbol_cargo.tga",
				hudMove            = "::/gui/line_vehicle_mgmt/icons/indicator_move.tga",
				hudAdd             = "::/gui/line_vehicle_mgmt/icons/add.tga",
				hudRemove          = "::/gui/line_vehicle_mgmt/icons/trash_bin.tga",
			},

			problemCacheState = problemCacheState,
			countDataState = countDataState,
			gamepadInputModeState = gamepadInputModeState,
			lvm2D3DModeState = lvm2D3DModeState,
			lvmSectionState = lvmSectionState,
			lvmActionButtonsState = lvmActionButtonsState,
			moreOptionsState = moreOptionsHoldState,
			sendToLineHoveredLineState = sendToLineHoveredLineState,
			addStopHoveredStationState = addStopHoveredStationState,
			preferLineOverTNEdgeState = preferLineOverTNEdgeState,
			creatingNewLineState = creatingNewLineState,
			gamepadNo3DSelectionInfoState = gamepadNo3DSelectionInfoState,

			vehicleManagerStateRef = toolParams.vehicleManagerStateRef,
			lineManagerStateRef = toolParams.lineManagerStateRef,
			vehicleManagerRef = vehicleManagerRef,
			lineAndDepotListRef = lineAndDepotListRef,
			vehicleStoreOpenForDepotAndLineRef = vehicleStoreOpenForDepotAndLineRef,
			deleteSellRef = deleteSellRef,
			bufferedSendDataRef = bufferedSendDataRef,
			lvmDnDDataRef = lvmDnDDataRef,

			configureLinesPopoverPosRef = configureLinesPopoverPosRef,
			sendToLinePopoverPosRef = sendToLinePopoverPosRef,
			btnBuyVehiclesRef = btnBuyVehiclesRef,
			btnSendToLineRef = btnSendToLineRef,
			btnNewLineRef = btnNewLineRef,

			switchMode = switchLVMMode,
			switchToPrevMode = switchToPrevLVMMode,
			getModeState = getLVMMode,
			addFeedback = function(
				message : string,
				mode : string,
				dialogData : FeedbackListUtil.FeedbackDialogParam,
				id : number)
				if feedbackViewerRef and feedbackViewerRef:get() then
					feedbackViewerRef:get():getApi().addFeedback(message, mode, dialogData, nil, id, true)
				end
			end,
			updateIA = function(newContext : LineUtil.GuiContextState)
				guiCtxForInputActions:transform(function(old : LineUtil.GuiContextState) : LineUtil.GuiContextState
					-- note: this is important, otherwise we run into infinite loop of one state change triggering the other!
					if line_util.guiContextEquals(old, newContext) then
						return old
					end
					return newContext
				end)
			end,
			accessNotificationCacheFn = toolParams.gameCtx.accessNotificationCacheFn,
			carrierFn = carrierFn,
			getDisableHudInteraction = function() : boolean
				local disableHudInteraction = gamepadInputModeState:old() and lvm2D3DModeState:old() == "2D"
				return disableHudInteraction
			end,
			resetLVMDnDData = function()
				lvmDnDDataRef:set({})
			end,
			getFirstStopToSendTo = function(
				vehicleToSendToLine : Engine.Entity,
				lineToSendTo : Engine.Entity
			) : integer
				local sendScheme = getSendScheme(
					{vehicleToSendToLine},
					toolParams.firstStopToSendSchemesRef,
					toolParams.firstStopToSendSchemeMapRef
				)

				local sendFn = util.useFn(sendScheme.firstStopToSendFnDefinition.fileName) as function(params : FirstStopToSend.SendParams) : {Engine.Entity : integer}

				local vehicle2stopIndex : {Engine.Entity : integer} = sendFn({
					vehicleEntities = {vehicleToSendToLine},
					lineEntity = lineToSendTo
				})

				return vehicle2stopIndex[vehicleToSendToLine]
			end,
		}
	end):get()

	local vehiclesToAssignToNewLineRef : ReactRefT<{Engine.Entity}> = react.useRef(nil)
	local madeNewLineRef : ReactRefT<boolean> = react.useRef(false)

	local retryBufferedData = function(lineEntity : Engine.Entity)
		for __, data in ipairs(bufferedSendDataRef:get()) do
			if api.engine.entityExists(data.vehicle)
				and api.engine.entityExists(data.line)
				and api.engine.entityExists(lineEntity) then

				if lineEntity == data.line then
					commonParams.sendVehiclesToLine({data.vehicle}, lineEntity)
				end
			end
		end
		bufferedSendDataRef:set({})
	end

	local forceUpdateCounter = react.useRef(0)
	local forceUpdateTag = react.useRef("0")

	-- This forces all react hud icons to rebuild (triggered on line change)
	-- It is done here so that not every recipe needs to have its own onStep to change
	local function forceIconUpdate()
		local counter = forceUpdateCounter:get() + 1
		forceUpdateCounter:set(counter)
		forceUpdateTag:set(tostring(counter))
	end

	local reactLineState, reactLineCommit : (ReactStateT<Line.ReactLine>) = engine_react_util.useStepState(
		function() : Line.ReactLine
			if #toolParams.lineManagerStateRef:get().lineListEntitiesSelected == 1 then
				local lineEAR = toolParams.lineManagerStateRef:get().lineListEntitiesSelected[1]
				if not entity_util.entityChanged0(lineEAR) then
					if entity_util.isOwnedByPlayer(lineEAR.entity) then
						return line_util.getReactLineFromGameState(lineEAR.entity)
					end
				end
			end

			if ((lvmModeState:old()[1] == "SEGMENT")
				or (lvmModeState:old()[1] == "MOVE_STOP")
				or (lvmModeState:old()[1] == "MOVE_WAYPOINT")) then

				switchLVMMode({"MAIN", nil})
			end
			return nil
		end,
		function(lineStateNew : Line.ReactLine) : Command<LineUpdateCommandData>, function(
			commandData : LineUpdateCommandData,
			success : boolean,
			affectedEntities : {{Engine.Entity, Engine.Revision}}
		)
			local function createLineCallback(success : boolean, lineEntity : Engine.Entity)
				blockNewLineState:set(false)

				if not success then
					log.error("Line update command failed")
					if not lineEntity then
						toolParams.onBack()
					end
				end

				local selectedLineEARs = toolParams.lineManagerStateRef:get().lineListEntitiesSelected
				local shouldScrollToLine = not ((#selectedLineEARs == 1) and (lineEntity == selectedLineEARs[1].entity))

				if madeNewLineRef:get() then
					-- TPF3-MP: the stops clicked meanwhile go on the new line.
					if success and lineEntity then
						queuedStopsLineRef:set(lineEntity)
					else
						queuedStopsRef:set({})
					end
					madeNewLineRef:set(false)
					toolParams.lineManagerStateRef:set(
						makeLineManagerState({entity_util.makeEntityAndRevision(lineEntity)}, {}, nil)
					)
					switchLVMMode({"SEGMENT", 1})
				end

				if vehiclesToAssignToNewLineRef:get() then
					local vmApi : VehicleManagerApi = nil
					if vehicleManagerRef:get() then
						vmApi = vehicleManagerRef:get():getApi()
					end

					if vmApi then
						vmApi.clearVehicleList()
						vmApi.addVehiclesToVehicleListAndSelect(vehiclesToAssignToNewLineRef:get())

						local sendScheme = getSendScheme(
							vehiclesToAssignToNewLineRef:get(),
							toolParams.firstStopToSendSchemesRef,
							toolParams.firstStopToSendSchemeMapRef
						)
						vmApi.sendVehiclesToLine(
							vehiclesToAssignToNewLineRef:get(),
							lineEntity,
							sendScheme,
							afterSendingToLineFixup
						)
					end
					vehiclesToAssignToNewLineRef:set(nil)
				end

				retryBufferedData(lineEntity)

				if shouldScrollToLine then
					scrollToNewLineRef:set(lineEntity)
				end
			end


			local cmd : Command<LineUpdateCommandData> = nil
			local callback : function(
				result : ICommandData,
				success : boolean,
				resultEntities : {{Engine.Entity, Engine.Revision}}
			) = nil


			local lineEntity : Engine.Entity = nil
			if lineStateNew.entityAndRevision then
				lineEntity = lineStateNew.entityAndRevision.entity
			end

			local newLineComp : Engine.Component.Line = api.type.Line.new()
			newLineComp.stops = line_util.autoAssignTerminals(lineEntity, lineStateNew.path)

			newLineComp.customFilters = lineStateNew.customFilters
			newLineComp.reservationPriority = lineStateNew.reservationPriority

			local oldEntity = lineEntity
			if lineEntity == nil then
				local playerEntity = api.engine.util.getPlayer()
				lineStateNew.color = line_util.makeLineColor(playerEntity)
				local name = line_util.makeLineName(playerEntity)
				cmd = api.cmd.makeLineCreateCmd(
					name,
					lineStateNew.color,
					playerEntity,
					newLineComp
				) as Command<LineUpdateCommandData>
				callback = function(
					_result : ICommandData,
					success : boolean,
					resultEntities : {{Engine.Entity, Engine.Revision}}
				)
					if success then
						createLineCallback(true, resultEntities[1][1])
					else
						createLineCallback(false, oldEntity) -- oldEntity is nil if the line did not exist yet
					end
				end
			else
				callback = function(_result : ICommandData, success : boolean)
					createLineCallback(success, oldEntity) -- update cannot change entity id
				end
				cmd = api.cmd.makeLineUpdateCmd(
					lineStateNew.entityAndRevision.entity,
					newLineComp
				)
			end

			return cmd, callback
		end,
		nil, -- no custom equals
		function()
			forceIconUpdate()
		end
	)

	local gamepadLineEntity = deleteSellRef:get().focusedLine
	local makeDeleteState = function() : integer
		if not gamepadLineEntity then
			return -1
		end
		if toolParams.gameCtx.filters:get().protectedEntities[gamepadLineEntity] then
			return -1
		end
		local vehicles = api.engine.system.transportVehicleSystem.getLineVehicles(gamepadLineEntity)
		for __, vehicleEntityId in ipairs(vehicles) do
			if toolParams.gameCtx.filters:get().protectedEntities[vehicleEntityId] then
				return -1
			end
		end
		return #vehicles
	end
	local makeDeleteCommands = function(numberOfVehicles : integer) : {{Command<ICommandData>, function(ICommandData, boolean, {{Engine.Entity, Engine.Revision}})}}
		local vehicles = api.engine.system.transportVehicleSystem.getLineVehicles(gamepadLineEntity)
		if #vehicles ~= numberOfVehicles then
			log.warning("Number of vehicles on line changed, no command has been created...")
			return nil
		end

		local missionError = api.gui.fireGuiScriptEvent("management.line", "delete", gamepadLineEntity) as boolean
		if missionError then
			return nil
		end

		local cmds : {{Command<ICommandData>, function(ICommandData, boolean, {{Engine.Entity, Engine.Revision}})}} = {}
		if #vehicles > 0 then
			for __, vehicleEntityId in ipairs(vehicles) do
				if toolParams.gameCtx.filters:get().protectedEntities[vehicleEntityId] then
					return nil
				end
			end
			local sellCmd = api.cmd.makeVehicleSellCmd(vehicles)
			cmds[#cmds + 1] = {sellCmd as Command<ICommandData>, nil}
		end

		local destroyCmd = api.cmd.makeLineDestroyCmd(gamepadLineEntity)
		cmds[#cmds + 1] = {destroyCmd as Command<ICommandData>, nil}

		return cmds
	end

	local deleteState, commitDelete : ReactStateT<integer>, function(integer) = engine_react_util.useStepStateMulti(makeDeleteState, makeDeleteCommands, nil, nil, true)
	local wrappedCommitDeleteGamepad = function()
		if api.engine.entityExists(gamepadLineEntity) then
			local ladlApi : LineAndDepotListApi = nil
			if lineAndDepotListRef:get() then
				ladlApi = lineAndDepotListRef:get():getApi()
			end

			if ladlApi then
				local wc = toolParams.gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				vehicleStoreOpenForDepotAndLineRef:set(nil)
				ladlApi.removeLineFromList(gamepadLineEntity, true)
				commonParams.switchMode{"MAIN", nil}
				commitDelete(makeDeleteState())
			end
		end
	end

	commonParams.lineState = reactLineState
	commonParams.forceUpdateTag = forceUpdateTag

	-- TPF3-MP: once the new line is loaded here, the stations clicked while it
	-- was being created are added to its end, in the order clicked, in one
	-- edit (queuedStopsRef).
	react.onStep(function()
		local waitedLine = queuedStopsLineRef:get()
		if not waitedLine then
			return
		end
		local current = reactLineState:old()
		if not (current and current.entityAndRevision and current.entityAndRevision.entity == waitedLine) then
			if not api.engine.entityExists(waitedLine) then
				queuedStopsLineRef:set(nil)
				queuedStopsRef:set({})
			end
			return
		end
		queuedStopsLineRef:set(nil)
		local queued = queuedStopsRef:get()
		queuedStopsRef:set({})
		if #queued == 0 then
			return
		end
		local lineState2 : Line.ReactLine = table_util.copy(current)
		local added = 0
		for __, station in ipairs(queued) do
			if not line_util.isStopProtected(waitedLine, station.stationGroup, gameCtx.filters:get().protectedEntities) then
				table.insert(lineState2.path, line_util.makeNewStop(station))
				added = added + 1
			end
		end
		if added == 0 then
			return
		end
		line_util.autoLoadConfig(lineState2)
		reactLineCommit(lineState2)
		commonParams.switchMode({"SEGMENT", #lineState2.path})
	end)

	commonParams.addDepotVehiclesToDepotVehicleCompare = function(depotEntity : Engine.Entity)
		depot2VehiclesCompareRef:transform(function(curr : {Engine.Entity : {Engine.Entity}}) : {Engine.Entity : {Engine.Entity}}
			local new = table_util.shallowCopy(curr)

			local depotVehicles = api.engine.system.transportVehicleSystem.getDepotVehicles(depotEntity)
			new[depotEntity] = depotVehicles

			return new
		end)
	end
	commonParams.removeDepotVehiclesFromDepotVehicleCompare = function(depotEntity : Engine.Entity)
		depot2VehiclesCompareRef:transform(function(curr : {Engine.Entity : {Engine.Entity}}) : {Engine.Entity : {Engine.Entity}}
			local new = table_util.shallowCopy(curr)

			new[depotEntity] = {}

			return new
		end)
	end
	commonParams.addVehicleToDepotVehicleCompare = function(vehicleEntity : Engine.Entity, depotEntity : Engine.Entity)
		depot2VehiclesCompareRef:transform(function(curr : {Engine.Entity : {Engine.Entity}}) : {Engine.Entity : {Engine.Entity}}
			local new : {Engine.Entity : {Engine.Entity}} = table_util.shallowCopy(curr)

			-- NOTE: does not check for duplicates (not necessary)
			table.insert(new[depotEntity], vehicleEntity)

			return new
		end)
	end
	commonParams.removeVehicleFromDepotVehicleCompare = function(vehicleEntity : Engine.Entity)
		depot2VehiclesCompareRef:transform(function(_curr : {Engine.Entity : {Engine.Entity}}) : {Engine.Entity : {Engine.Entity}}
			local new : {Engine.Entity : {Engine.Entity}} = {}

			for depot, vehicles in orderedPairs(depot2VehiclesCompareRef:get()) do
				if vehicles then
					local vehiclesNew : {Engine.Entity} = {}
					for __, vehicle in ipairs(vehicles as {Engine.Entity}) do
						if vehicle ~= vehicleEntity then
							table.insert(vehiclesNew, vehicle)
						end
					end
					new[depot as Engine.Entity] = vehiclesNew
				end
			end

			return new
		end)
	end
	commonParams.clearDepotVehicleCompare = function()
		depot2VehiclesCompareRef:set({})
	end

	commonParams.newLine = function(
		newLineStartingWithStation : LineUtil.StationSelectionDetails,
		vehicleEntities : {Engine.Entity}
	)
		if blockNewLineState:old() then
			-- TPF3-MP: added to the line once it exists (queuedStopsRef).
			local queued = queuedStopsRef:get()
			queued[#queued + 1] = newLineStartingWithStation
			queuedStopsRef:set(queued)
			return
		end
		queuedStopsRef:set({})
		queuedStopsLineRef:set(nil)

		creatingNewLineState:set(false)
		blockNewLineState:set(true)
		madeNewLineRef:set(true)
		local stop = line_util.makeNewStop(newLineStartingWithStation)
		local lineState2 : Line.ReactLine = {
			path = { stop },
			reservationPriority = 1.0,
		}
		if vehicleEntities and #vehicleEntities > 0 then
			vehiclesToAssignToNewLineRef:set(vehicleEntities)
		end
		toolParams.lineManagerStateRef:set(makeLineManagerState({}, {}, nil))

		line_util.autoLoadConfig(lineState2)

		reactLineCommit(lineState2)
	end
	commonParams.selectLine = function(
		line : LineUtil.LineSelectionDetails,
		scrollToRow : boolean,
		selectSegment : integer
	)
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end
		local ladlApi : LineAndDepotListApi = nil
		if lineAndDepotListRef:get() then
			ladlApi = lineAndDepotListRef:get():getApi()
		end

		retryBufferedData(line.entity)

		if vmApi and ladlApi then
			selectSingleLineAndOverrideSelectionOnClick(
				vmApi,
				ladlApi,
				creatingNewLineState,
				line.entity,
				false,
				selectSegment
			)
			if scrollToRow then
				ladlApi.scrollToRow(line.entity, true)
			end
		end

		if commonParams.gamepadInputModeState:old() then
			lvm2D3DModeState:set("2D") -- 4/4 way to exit 3D
			lvmExit3DControllerFocusRef:set(2)
		end
	end
	commonParams.deleteGamepadFocusedLine = function()
		if deleteState:old() == -1 then
			commonParams.addFeedback(_("Cannot delete line."), "Error", nil, 1)
		elseif deleteState:old() > 0 then
			commonParams.addFeedback(_("Deleting the line will sell all vehicles on the line."), "Question", {
				onAccept = function()
					wrappedCommitDeleteGamepad()
				end,
				acceptText = _("Delete"),
				soundOnAccept = "DeleteLine",
			}, 2)
		else
			wrappedCommitDeleteGamepad()
		end
	end

	commonParams.addStop = function(
		station : LineUtil.StationSelectionDetails,
		insertAtStopNumber : integer
	)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())

		if line_util.isStopProtected(lineState2.entityAndRevision.entity, station.stationGroup, gameCtx.filters:get().protectedEntities) then
			commonParams.addFeedback(_("Cannot add stop."), "Error", nil, 1)
			return
		end

		local insertFirst = 1
		local insertLast = lineState2 and (#lineState2.path + 1) or 1
		local insertAt : integer = insertAtStopNumber and insertAtStopNumber + 1 or insertLast
		insertAt = math_util.clamp(insertAt, insertFirst, insertLast)

		local newStop = line_util.makeNewStop(station)
		table.insert(lineState2.path, insertAt, newStop)

		line_util.autoLoadConfig(lineState2)

		reactLineCommit(lineState2)
		commonParams.switchMode({"SEGMENT", insertAt})
	end
	commonParams.removeStop = function(stop : LineUtil.StopSelectionDetails)
		local stationGroupEntity = (stop.station and stop.station.stationGroup) or nil
		if line_util.isStopProtected(stop.line, stationGroupEntity, gameCtx.filters:get().protectedEntities) then
			commonParams.addFeedback(_("Cannot delete stop."), "Error", nil, 1)
			return
		end

		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())
		table.remove(lineState2.path, stop.stopNumber)

		local willDeleteLine : boolean = true
		if commonParams.lineState:old().path then
			for k, via in ipairs(commonParams.lineState:old().path) do
				if stop.stopNumber ~= k then
					if via.stop then
						willDeleteLine = false
					end
				end
			end
		end

		if willDeleteLine then
			if not stop.line then
				return
			end
			local vehicles = api.engine.system.transportVehicleSystem.getLineVehicles(stop.line)
			local missionError = api.gui.fireGuiScriptEvent("management.line", "delete", stop.line) as boolean
			if missionError then
				commonParams.addFeedback(_("Cannot delete line."), "Error", nil, 1)
				return
			elseif #vehicles == 0 then
				local ladlApi : LineAndDepotListApi = nil
				if lineAndDepotListRef:get() then
					ladlApi = lineAndDepotListRef:get():getApi()
				end

				if ladlApi then
					ladlApi.removeLineFromList(stop.line, true)
				end
				local cmd = api.cmd.makeLineDestroyCmd(stop.line)
				api.cmd.sendCommand(cmd, function(__result : ICommandData, __success : boolean, __resultEntities : {{Engine.Entity, Engine.Revision}})
				end)
				local wc = gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				vehicleStoreOpenForDepotAndLineRef:set(nil)
				makeLineManagerState({}, {}, stop.line) -- clear selection but do not "re-add" deleted line
				switchLVMMode({"MAIN", nil})
				return
			else
				commonParams.addFeedback(_("Cannot delete the line because vehicles are assigned to it."), "Warning", nil, 0)
				log.message("Cannot remove last stop of a line (thus deleting the line), there are still vehicles assigned")
				return
			end
		else
			line_util.autoLoadConfig(lineState2)
			switchLVMMode({"SEGMENT", stop.stopNumber - 1})
		end

		reactLineCommit(lineState2)
	end
	commonParams.moveStop = function(movingStop : LineUtil.StopSelectionDetails, station : LineUtil.StationSelectionDetails)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())
		if movingStop.station.stationGroup ~= station.stationGroup then
			lineState2.path[movingStop.stopNumber].stop.alternativeTerminals = nil
		end
		lineState2.path[movingStop.stopNumber].stop.stationGroup = station.stationGroup
		lineState2.path[movingStop.stopNumber].stop.station1 = station.stationIndex1
		lineState2.path[movingStop.stopNumber].stop.terminal1 = station.terminalIndex1

		reactLineCommit(lineState2)
		api.gui.sound.playRandomSoundEffect(lvmSFX.MoveStation)
		if commonParams.gamepadInputModeState:old() then
			lvm2D3DModeState:set("2D")
			commonParams.switchMode({"MAIN", nil})
		else
			commonParams.switchMode({"SEGMENT", nil})
		end
	end
	commonParams.moveVia = function(oldStopNumber : integer, newStopNumber : integer)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())
		local via = lineState2.path[oldStopNumber]

		newStopNumber = math_util.clamp(newStopNumber, 1, #lineState2.path)
		if oldStopNumber ~= newStopNumber then
			local less = newStopNumber < oldStopNumber
			table.insert(lineState2.path, newStopNumber + (less and 0 or 1), via)
			table.remove(lineState2.path, oldStopNumber + (less and 1 or 0))

			line_util.autoLoadConfig(lineState2)

			reactLineCommit(lineState2)
			commonParams.switchMode({"SEGMENT", newStopNumber})
		end
	end
	commonParams.addAlternativeTerminal = function(details : LineUtil.StationSelectionDetails)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())

		local stopNumber : integer = (commonParams.getModeState()[2] as LineUtil.StopSelectionDetails).stopNumber as integer

		table.insert(lineState2.path[stopNumber].stop.alternativeTerminals, api.type.StationTerminal.new(details.stationIndex1 - 1, details.terminalIndex1 - 1))
		reactLineCommit(lineState2)
	end
	commonParams.removeAlternativeTerminal = function(details : LineUtil.StationSelectionDetails)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())

		local stopNumber : integer = (commonParams.getModeState()[2] as LineUtil.StopSelectionDetails).stopNumber as integer

		local newAlternatives = {}
		for __, v in ipairs(lineState2.path[stopNumber].stop.alternativeTerminals) do
			if v.station ~= details.stationIndex1 - 1 as integer or v.terminal ~= details.terminalIndex1 - 1 then
				table.insert(newAlternatives, api.type.StationTerminal.new(v.station, v.terminal))
			end
		end

		lineState2.path[stopNumber].stop.alternativeTerminals = newAlternatives
		reactLineCommit(lineState2)
	end
	commonParams.selectAlternativeTerminal = function(
		stopNumber : integer,
		stationIndex1 : integer,
		terminalIndex1 : integer,
		add : boolean
	)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())

		if add then
			table.insert(lineState2.path[stopNumber].stop.alternativeTerminals,
				api.type.StationTerminal.new(stationIndex1 - 1, terminalIndex1 - 1))
		else
			local newAlternatives = {}
			for __, v in ipairs(lineState2.path[stopNumber].stop.alternativeTerminals) do
				if v.station ~= stationIndex1 - 1 as integer or v.terminal ~= terminalIndex1 - 1 then
					table.insert(newAlternatives, api.type.StationTerminal.new(v.station, v.terminal))
				end
			end
			lineState2.path[stopNumber].stop.alternativeTerminals = newAlternatives
		end

		reactLineCommit(lineState2)
	end
	commonParams.changeMainTerminal = function(
		stopNumber : integer,
		stationIndex1 : integer,
		terminalIndex1 : integer
	)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())
		lineState2.path[stopNumber].stop.station1 = stationIndex1
		lineState2.path[stopNumber].stop.terminal1 = terminalIndex1

		reactLineCommit(lineState2)
	end
	commonParams.addWaypoint = function(
		waypoint : Waypoint,
		insertAtStopNumber : integer
	)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())

		if line_util.isStopProtected(lineState2.entityAndRevision.entity, nil, gameCtx.filters:get().protectedEntities) then
			commonParams.addFeedback(_("Cannot add waypoint."), "Error", nil, 1)
			return
		end

		local insertAt : integer = insertAtStopNumber and insertAtStopNumber + 1 or math.max(lineState2 and #lineState2.path or 0, 1)
		local wpClone = waypoint:clone()
		if wpClone.tag < 0 then
			wpClone.tag = lineState2.maxWaypointTag + 1
		end
		if wpClone.tag > lineState2.maxWaypointTag then
			lineState2.maxWaypointTag = wpClone.tag
		end
		table.insert(lineState2.path, insertAt, { waypoint = wpClone })

		line_util.autoLoadConfig(lineState2)

		reactLineCommit(lineState2)
		commonParams.switchMode({"SEGMENT", insertAt})
	end
	commonParams.removeWaypoint = function(waypoint : LineUtil.WaypointSelectionDetails)
		if gameCtx.filters:get().protectedEntities[waypoint.line] == true then
			commonParams.addFeedback(_("Cannot delete waypoint."), "Error", nil, 1)
			return
		end
		local lineState2 : Line.ReactLine = table_util.shallowCopy(commonParams.lineState:old())
		local oldPath = commonParams.lineState:old().path
		local newPath : {Line.ReactVia} = {}
		local insertAt = 0
		for stopNumber, via in ipairs(oldPath) do
			if via.waypoint == nil or via.waypoint.tag ~= waypoint.tag then
				newPath[#newPath + 1] = via
			else
				insertAt = stopNumber - 1
			end
		end
		lineState2.path = newPath
		switchLVMMode({"SEGMENT", insertAt})
		reactLineCommit(lineState2)
	end
	commonParams.moveWaypoint = function(movingWaypoint : LineUtil.WaypointSelectionDetails, apiWaypoint : Waypoint)
		local lineState2 : Line.ReactLine = table_util.copy(commonParams.lineState:old())
		lineState2.path[movingWaypoint.stopNumber].waypoint = apiWaypoint

		reactLineCommit(lineState2)
		api.gui.sound.playRandomSoundEffect(lvmSFX.MoveWaypoint)
		if commonParams.gamepadInputModeState:old() then
			lvm2D3DModeState:set("2D")
		end
		commonParams.switchMode({"SEGMENT", nil})
	end

	commonParams.addOrRemoveVehicleToVehicleList = function(vehicleDepotDetails : LineUtil.VehicleDepotDetails)
		if creatingNewLineState:old() then
			creatingNewLineState:set(false)
			switchLVMMode({"MAIN", nil})
		end
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			if vehicleDepotDetails.removeFromList then
				vmApi.removeVehiclesFromVehicleList({vehicleDepotDetails.entity})
			else
				vmApi.addVehiclesToVehicleListAndSelect({vehicleDepotDetails.entity})
			end
		end
	end
	commonParams.addOrRemoveDepotVehiclesToVehicleList = function(vehicleDepotDetails : LineUtil.VehicleDepotDetails)
		if creatingNewLineState:old() then
			creatingNewLineState:set(false)
			switchLVMMode({"MAIN", nil})
		end
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			local depotVehicles = api.engine.system.transportVehicleSystem.getDepotVehicles(vehicleDepotDetails.entity)

			if vehicleDepotDetails.removeFromList then
				vmApi.removeVehiclesFromVehicleList(depotVehicles)
			else
				vmApi.addVehiclesToVehicleListAndSelect(depotVehicles)
			end
		end
	end
	commonParams.addOrRemoveLineVehiclesToVehicleList = function(
		lineEntity : Engine.Entity,
		removeFromList : boolean,
		clearList : boolean
	)
		if creatingNewLineState:old() then
			creatingNewLineState:set(false)
			switchLVMMode({"MAIN", nil})
		end
		if lineEntity and vehicleManagerRef then
			local vmApi : VehicleManagerApi = nil
			if vehicleManagerRef:get() then
				vmApi = vehicleManagerRef:get():getApi()
			end

			if vmApi then
				if clearList then
					vmApi.clearVehicleList()
				end
				local vehiclesOnLine = api.engine.system.transportVehicleSystem.getLineVehicles(lineEntity)
				for __, vehicleEntity in ipairs(vehiclesOnLine) do
					if removeFromList then
						vmApi.removeVehiclesFromVehicleList({vehicleEntity})
					else
						vmApi.addVehiclesToVehicleListAndSelect({vehicleEntity})
					end
				end
			end
		end
	end
	commonParams.isVehicleSelected = function(vehicleEntity : Engine.Entity) : boolean
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		local isSelected = false
		if vmApi then
			isSelected = vmApi.isVehicleSelected(vehicleEntity)
		end
		return isSelected
	end
	commonParams.isVehicleInVehicleList = function(vehicleEntity : Engine.Entity) : boolean
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			return vmApi.isVehicleInVehicleList(vehicleEntity)
		end
		return false
	end
	commonParams.selectOrDeselectVehicleInVehicleList = function(
		vehicleEntity : Engine.Entity,
		selectVehicle : boolean
	)
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			vmApi.selectVehicles({vehicleEntity}, selectVehicle)
		end
	end
	commonParams.areAllDepotVehiclesInVehicleList = function(vehicleDepotEntity : Engine.Entity) : boolean
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			local depotVehicles = api.engine.system.transportVehicleSystem.getDepotVehicles(vehicleDepotEntity)

			if #depotVehicles == 0 then
				return false
			end

			for __, vehicleEntity in ipairs(depotVehicles) do
				local alreadyPresent = vmApi.isVehicleInVehicleList(vehicleEntity)
				if not alreadyPresent then
					return false
				end
			end
		end
		return true
	end

	commonParams.openVehicleDepot = function(
		depotEntity : Engine.Entity,
		lineEntity : Engine.Entity
	)
		moreOptionsHoldState:set(false)
		lvm2D3DModeState:set("2D") -- 3/4 way to exit 3D

		local nameCmp = api.engine.util.getEntityName(depotEntity)

		vehicleStoreEventParamState:set({
			title = lang_util.format(_("Buy Vehicles At {name}"), {
				name = nameCmp,
			}),
			depot = depotEntity,
			carrier = vehicle_util.getDepotCarrier(depotEntity),
			transportModes = vehicle_util.getDepotTransportModes(depotEntity),
			filterTags = vehicle_store_util.getDepotFilterTags(depotEntity),
			getFirstStopToSendTo = commonParams.getFirstStopToSendTo,
			onLocate = function()
				api.gui.camera.followEntity(depotEntity, true)
			end,
			onClose = function()
				local wc = gameCtx.windowContainer:get():getApi()
				wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
				vehicleStoreOpenForDepotAndLineRef:set(nil)
				lvmExit3DControllerFocusRef:set(2)
			end,
			buyCallback = function(changes : {{Engine.Entity, Engine.Revision}})
				local vmApi : VehicleManagerApi = nil
				if vehicleManagerRef:get() then
					vmApi = vehicleManagerRef:get():getApi()
				end

				if vmApi then
					for __, vehicleER in ipairs(changes) do
						vmApi.addVehiclesToVehicleListAndSelect({vehicleER[1]})
					end

					if lineEntity then
						local vehicles : {Engine.Entity} = {}
						for __, vehicleER in ipairs(changes) do
							table.insert(vehicles, vehicleER[1])
						end
						local sendScheme = getSendScheme(
							vehicles,
							toolParams.firstStopToSendSchemesRef,
							toolParams.firstStopToSendSchemeMapRef
						)

						vmApi.sendVehiclesToLine(
							vehicles,
							lineEntity,
							sendScheme,
							afterSendingToLineFixup
						)
					end
				end
			end
		})

		vehicleStoreOpenForDepotAndLineRef:set({
			depot = depotEntity,
			line = lineEntity,
			vehicleStoreMode = "Buy",
		})
	end
	commonParams.areAnyVehiclesInDepot = function(vehicleDepotEntity : Engine.Entity) : boolean
		local depotVehicles = api.engine.system.transportVehicleSystem.getDepotVehicles(vehicleDepotEntity)
		return #depotVehicles ~= 0
	end
	commonParams.selectOrDeselectDepot = function(vehicleDepotDetails : LineUtil.VehicleDepotDetails)
		local ladlApi : LineAndDepotListApi = nil
		if lineAndDepotListRef:get() then
			ladlApi = lineAndDepotListRef:get():getApi()
		end

		if ladlApi then
			ladlApi.selectDepot(vehicleDepotDetails.entity, not vehicleDepotDetails.removeFromList)
		end
	end

	commonParams.sendVehiclesToLine = function(
		selection : {Engine.Entity},
		lineEntity : Engine.Entity
	)
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			local sendScheme = getSendScheme(
				selection,
				toolParams.firstStopToSendSchemesRef,
				toolParams.firstStopToSendSchemeMapRef
			)
			vmApi.sendVehiclesToLine(
				selection,
				lineEntity,
				sendScheme,
				afterSendingToLineFixup
			)
		end
	end
	commonParams.sendVehiclesToDepot = function(
		selection : {Engine.Entity},
		depotEntity : Engine.Entity
	)
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			vmApi.sendVehiclesToDepot(selection, depotEntity)
		end
		switchLVMMode({"MAIN", nil})
	end

	commonParams.openWarehouse = function(warehouseEntity : Engine.Entity)
		local wc = gameCtx.windowContainer:get():getApi()
		wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
		vehicleStoreOpenForDepotAndLineRef:set(nil)

		react.fireEvent(nil, "selectEntity", {entity = warehouseEntity, stack = true})
	end
	commonParams.sellVehicle = function(vehicleEntity : Engine.Entity)
		local vmApi : VehicleManagerApi = nil
		if vehicleManagerRef:get() then
			vmApi = vehicleManagerRef:get():getApi()
		end

		if vmApi then
			api.cmd.sendCommand(
				api.cmd.makeVehicleSellCmd({vehicleEntity}))
			vmApi.removeVehiclesFromVehicleList({vehicleEntity})
		end
	end
	commonParams.openCargoFilter = function(
		where : LineUtil.StopSelectionDetails,
		pos : Vec2f, onClose : function()
	)
		local cargoFilterParams : CargoFilterUtil.CargoFilterParams = {}
		cargoFilterParams.onClose = function()
			gameCtx.windowContainer:get():getApi().removeAllWindows(CargoFilterWindow)

			if onClose then
				onClose()
			end
		end
		if where.line == nil then
			return
		end
		cargoFilterParams.lineEntity = where.line
		cargoFilterParams.stopIndex = where.apiStopIndex0
		cargoFilterParams.stopNumber = where.stopNumber
		cargoFilterParams.gameCtx = gameCtx
		cargoFilterParams.pos = pos + api.type.Vec2f.new(0.002, -0.4)
		gameCtx.windowContainer:get():getApi().addSingletonWindow(CargoFilterWindow, cargoFilterParams)
	end
	commonParams.closeCargoFilter = function()
		gameCtx.windowContainer:get():getApi().removeAllWindows(CargoFilterWindow)
	end

	commonParams.btnNewLineEnabled = function() : boolean
		local newLineEnabled = (not creatingNewLineState:old()) and (not gameCtx.disableFeatures:get()["CreateNewLine"])

		return newLineEnabled
	end
	commonParams.btnNewLineOnClick = function()
		params.toolParam.lineManagerStateRef:set(makeLineManagerState({}, {}, nil))
		params.toolParam.vehicleManagerStateRef:set(makeVehicleManagerState({}, {}))
		creatingNewLineState:set(true)
		moreOptionsHoldState:set(false)
		lvm2D3DModeState:set("3D") -- 2/4 way to enter 3D
		lvmActionButtonsState:set("NONE")
		switchLVMMode({"MAIN", nil})
	end
	commonParams.btnSendToLineEnabled = function() : boolean
		local vehicleEARSelection : { EntityUtil.EntityAndRevision } = params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesSelected
		local sendToLineEnabled = #vehicleEARSelection > 0 and not sendToLineDisabledState:old()

		return sendToLineEnabled
	end
	commonParams.btnSendToLineOnClick = function()
		sendToLineDisabledState:set(true)
		if sendToLineDisabledState:old() then
			return
		end

		local vehicleEARSelection : { EntityUtil.EntityAndRevision } = params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesSelected

		local vehicleEntities : {Engine.Entity} = {}
		for __, vehicleEAR in ipairs(vehicleEARSelection) do
			if not entity_util.entityChanged0(vehicleEAR) then
				table.insert(vehicleEntities, vehicleEAR.entity)
			end
		end

		local function getWindowApi() : Builtin.WindowAPI
			return params.toolParam.gameCtx.windowContainer:get():getApi()
		end

		local wc = getWindowApi()
		wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
		vehicleStoreOpenForDepotAndLineRef:set(nil)

		local pos = sendToLinePopoverPosRef:get():getPosition(1.0, 1.0)
		wc.removeAllWindows(popover_react_util.PopoverWindow)
		wc.addWindow(popover_react_util.PopoverWindow, "send-to-line", {
			onClose = function()
				cleanupSendToLine(
					gameCtx,
					sendToLineDisabledState,
					switchToPrevLVMMode
				)
			end,
			x = pos.x,
			y = pos.y,
			windowTitle = _("Send To Line"),
			windowClass = "select-line, management",
			recipe = SendToLineSelection as IRecipe,
			params = {
				commonParams = commonParams,
				selection = vehicleEntities,
				sendToLine = function(lineEntity : Engine.Entity, sendScheme : FirstStopToSend.Scheme)
					sendVehiclesToLineCommands(
						vehicleEntities,
						lineEntity,
						sendScheme,
						afterSendingToLineFixup,
						bufferedSendDataRef,
						true -- doNotBuffer
					)
					cleanupSendToLine(
						gameCtx,
						sendToLineDisabledState,
						switchToPrevLVMMode
					)
				end,
				onlyVisible = false,
				firstStopToSendSchemesRef = params.toolParam.firstStopToSendSchemesRef,
				firstStopToSendSchemeMapRef = params.toolParam.firstStopToSendSchemeMapRef,
			},
			inputActionBubbleUpFilter = builtin.type.InputActionBubbleUpFilter.filterOut{
				"IA_CAMPAIGN_MISSION_MENU_OPEN",
			}
		})

		switchLVMMode({"SEND_TO_LINE", nil})
	end
	commonParams.btnBuyVehiclesEnabled = function() : boolean
		local singleLine : Engine.Entity = nil
		if reactLineState and reactLineState:old() then
			if reactLineState:old().entityAndRevision then
				singleLine = reactLineState:old().entityAndRevision.entity
			end
		end
		local depotsSelected = params.toolParam.lineManagerStateRef:get().depotListEntitiesSelected
		local singleDepot = (depotsSelected and #depotsSelected > 0) and depotsSelected[1].entity or nil
		local buyVehiclesEnabled = (singleDepot ~= nil) or (singleLine ~= nil)

		return buyVehiclesEnabled
	end
	commonParams.btnBuyVehiclesOnClick = function()
		local singleLine : Engine.Entity = nil
		if reactLineState and reactLineState:old() then
			if reactLineState:old().entityAndRevision then
				singleLine = reactLineState:old().entityAndRevision.entity
			end
		end

		if singleLine then
			local depotForLine = line_util.getBestDepotForLine(singleLine)
			if depotForLine and depotForLine >= 0 then
				openDepotWindow(
					gameCtx,
					depotForLine,
					singleLine,
					vehicleStoreOpenForDepotAndLineRef,
					commonParams.openVehicleDepot
				)
			else
				commonParams.addFeedback(_("Cannot find suitable depot reachable from the line."), "Warning", nil, 1)
			end
		else
			local depotsSelected = params.toolParam.lineManagerStateRef:get().depotListEntitiesSelected
			local singleDepot = depotsSelected[1].entity

			openDepotWindow(
				gameCtx,
				singleDepot,
				nil,
				vehicleStoreOpenForDepotAndLineRef,
				commonParams.openVehicleDepot
			)
		end
	end

	local controllerStyleClasses : {string} = {}
	local fn : (function (CommonActionParams, any)  : TreeNodeId) = nil

	if lvmModeState:old()[1] == "SEGMENT" then
		fn = makeActionSegmentMode
		controllerStyleClasses[#controllerStyleClasses + 1] = "actionSegment"
	elseif lvmModeState:old()[1] == "MOVE_STOP" then
		fn = makeActionMoveStopMode
		controllerStyleClasses[#controllerStyleClasses + 1] = "actionMoveStop"
	elseif lvmModeState:old()[1] == "MOVE_WAYPOINT" then
		fn = makeActionMoveWaypointMode
		controllerStyleClasses[#controllerStyleClasses + 1] = "actionMoveWaypoint"
	elseif lvmModeState:old()[1] == "SEND_TO_LINE" then
		fn = makeActionSendVehiclesMode
		controllerStyleClasses[#controllerStyleClasses + 1] = "actionSendToLine"
	else -- lvmModeState:old()[1] == "MAIN"
		fn = makeActionMainMode
		controllerStyleClasses[#controllerStyleClasses + 1] = "actionMain"
	end

	local makeActionFn = function() : (function() : TreeNodeId)
		return function() : TreeNodeId return fn(commonParams, lvmModeState:old()[2]) end
	end
	params.setActionFn(makeActionFn(), lvmModeState:old()[1])

	if not commonParams.gamepadInputModeState:old() and
			not moreOptionsHoldState:old() and
			(#toolParams.lineManagerStateRef:get().lineListEntitiesSelected > 0) then
		react.useInputAction(
			"IA_MENU_BACK", -- also used with more options in 2D
			react.iaHandler(
				function()
					toolParams.lineManagerStateRef:set(
						makeLineManagerState({}, {}, nil)
					)
					toolParams.vehicleManagerStateRef:set(
						makeVehicleManagerState({}, {})
					)
					switchLVMMode({"MAIN", nil})
				end,
				isEnabledReturnTrue,
				nGetText(
					"Deselect Line",
					"Deselect Lines",
					#toolParams.lineManagerStateRef:get().lineListEntitiesSelected
				)
			)
		)
	elseif creatingNewLineState:old() then
		controllerStyleClasses[#controllerStyleClasses + 1] = "newLine"
		react.useInputAction(
			"IA_MENU_BACK", -- also used with more options in 2D
			react.iaHandler(
				function()
					creatingNewLineState:set(false)
					switchLVMMode({"MAIN", nil})
					lvm2D3DModeState:set("2D")
				end,
				isEnabledReturnTrue,
				_("Cancel Creating New Line")
			)
		)
	end

	local nextSectionEnabledState : ReactStateT<boolean> = engine_react_util.useStepState(function() : boolean
		local lineSelected = #toolParams.lineManagerStateRef:get().lineListEntitiesSelected > 0
		local depotSelected = #toolParams.lineManagerStateRef:get().depotListEntitiesSelected > 0
		local vehicleSelected = #toolParams.vehicleManagerStateRef:get().vehicleListEntitiesSelected > 0 or
				#toolParams.vehicleManagerStateRef:get().vehicleListEntitiesUnselected > 0

		return lineSelected or depotSelected or vehicleSelected
	end)

	local fnToLM = function()
		if lineAndDepotListRef:get() then
			lineAndDepotListRef:get():focus()
			lvmSectionState:set("LM")
		end
	end
	local fnToVMNext = function()
		if vehicleListRef:get() then
			vehicleListRef:get():focus()
			lvmSectionState:set("VM")
		elseif linePanelRef:get() then
			linePanelRef:get():focus()
			lvmSectionState:set("LP")
		elseif linePanelActionButtonsRef:get() then -- skip VM if a depot/line without vehicles is selected
			linePanelActionButtonsRef:get():focus()
			lvmSectionState:set("LP")
		end
	end
	local fnToLP = function()
		if linePanelRef:get() then
			linePanelRef:get():focus()
			lvmSectionState:set("LP")
		elseif linePanelActionButtonsRef:get() and
			#toolParams.lineManagerStateRef:get().depotListEntitiesSelected > 0 then

			linePanelActionButtonsRef:get():focus()
			lvmSectionState:set("LP")
		elseif lineAndDepotListRef:get() then
			lineAndDepotListRef:get():focus()
			lvmSectionState:set("LM")
		end
	end

	if gamepadInputModeState:old() then
		if lvm2D3DModeState:old() == "2D" then
			controllerStyleClasses[#controllerStyleClasses + 1] = "manager2DMode"

			local fnToABLM = function()
				if lineManagerActionButtonsRef:get() then
					lineManagerActionButtonsRef:get():focus()
				end
			end
			local fnToABVM = function()
				if vehicleManagerActionButtonsRef:get() then
					vehicleManagerActionButtonsRef:get():focus()
				end
			end
			local fnToABLP = function()
				if linePanelActionButtonsRef:get() then
					linePanelActionButtonsRef:get():focus()
				end
			end

			local fnNextSection = function() end
			local fnFocusActionButtons = function() end

			if lvmSectionState:old() == "LM" then
				fnNextSection = fnToVMNext
				fnFocusActionButtons = fnToABLM
			elseif lvmSectionState:old() == "VM" then
				fnNextSection = fnToLP
				fnFocusActionButtons = fnToABVM
			else -- lvmSectionState:old() == "LP"
				fnNextSection = fnToLM
				fnFocusActionButtons = fnToABLP
			end

			react.useInputAction(
				commonParams.lvmButtonIAs.btn2D_MoreOptions,
				react.iaHandlerExtended(
					function(data : Gui.InputAction.InvokeData)
						if data.status == api.gui.inputAction.InvokeData.Status.Triggered then
							moreOptionsHoldState:set(true)
						elseif data.status == api.gui.inputAction.InvokeData.Status.Repeat then
							moreOptionsHoldState:set(true)
						elseif data.status == api.gui.inputAction.InvokeData.Status.EndReleased then
							moreOptionsHoldState:set(false)
						elseif data.status == api.gui.inputAction.InvokeData.Status.EndAborted then
							moreOptionsHoldState:set(false)
						end
					end
				)
			)

			if nextSectionEnabledState:old() then
				react.useInputAction(
					commonParams.lvmButtonIAs.btn2D_NextSection,
					react.iaHandler(fnNextSection)
				)
			end

			if moreOptionsHoldState:old() then
				controllerStyleClasses[#controllerStyleClasses + 1] = "manager2DMoreOptions"

				react.useInputAction(
					commonParams.lvmButtonIAs.btnMore_NewLine,
					react.iaForward(commonParams.btnNewLineRef, "ACTION_CLICK")
				)

				react.useInputAction(
					commonParams.lvmButtonIAs.btnMore_SendToLine,
					react.iaForward(commonParams.btnSendToLineRef, "ACTION_CLICK")
				)

				react.useInputAction(
					commonParams.lvmButtonIAs.btnMore_BuyVehicle,
					react.iaForward(commonParams.btnBuyVehiclesRef, "ACTION_CLICK")
				)

				local tooltipDeleteSell : string = nil
				local fnDeleteSellIsEnabled = function() : boolean return false end
				local fnDeleteSell = function() end
				if lvmSectionState:old() == "LM" then
					tooltipDeleteSell = _("Delete Line")
					fnDeleteSellIsEnabled = function() : boolean
						local focusedLine = deleteSellRef:get().focusedLine
						if focusedLine == nil then
							return false
						end
						if not api.engine.entityExists(focusedLine) then
							return false
						end

						local lineComp : Engine.Component.Line = api.engine.getComponent(focusedLine, api.type.ComponentType.LINE)
						return lineComp ~= nil
					end
					fnDeleteSell = function()
						commonParams.deleteGamepadFocusedLine()
						api.gui.sound.playRandomSoundEffect(lvmSFX.DeleteLine)
						deleteSellRef:set({
							focusedLine = nil,
							focusedVehicle = nil,

							focusedStopLine = nil,
							focusedStopNumber = nil,
							focusedApiStopIndex0 = nil,
						})
					end
				elseif lvmSectionState:old() == "VM" then
					tooltipDeleteSell = _("Sell Vehicle")
					fnDeleteSellIsEnabled = function() : boolean
						return deleteSellRef:get().focusedVehicle ~= nil
					end
					fnDeleteSell = function()
						commonParams.sellVehicle(deleteSellRef:get().focusedVehicle)
						api.gui.sound.playRandomSoundEffect(lvmSFX.SellVehicle)
						deleteSellRef:set({
							focusedLine = nil,
							focusedVehicle = nil,

							focusedStopLine = nil,
							focusedStopNumber = nil,
							focusedApiStopIndex0 = nil,
						})
					end
				else -- lvmSectionState:old() == "LP"
					tooltipDeleteSell = _("Delete Stop")
					fnDeleteSellIsEnabled = function() : boolean
						return (deleteSellRef:get().focusedStopLine ~= nil
							and deleteSellRef:get().focusedStopNumber ~= nil)
							-- apiStopIndex is nil for waypoints
					end
					fnDeleteSell = function()
						commonParams.removeStop({
							line = deleteSellRef:get().focusedStopLine,
							stopNumber = deleteSellRef:get().focusedStopNumber,
							apiStopIndex0 = deleteSellRef:get().focusedApiStopIndex0,
						})
						api.gui.sound.playRandomSoundEffect(lvmSFX.RemoveStation)
						deleteSellRef:set({
							focusedLine = nil,
							focusedVehicle = nil,

							focusedStopLine = nil,
							focusedStopNumber = nil,
							focusedApiStopIndex0 = nil,
						})
					end
				end

				react.useInputAction(
					commonParams.lvmButtonIAs.btnMore_DeleteSell,
					react.iaHandler(
						fnDeleteSell,
						fnDeleteSellIsEnabled,
						tooltipDeleteSell
					)
				)
			else
				react.useInputAction( -- 1/4 way to enter 3D
					commonParams.lvmButtonIAs.btn2D3D_Switch,
					react.iaHandler(
						function()
							lvm2D3DModeState:set("3D")
							creatingNewLineState:set(false)
							moreOptionsHoldState:set(false)
							if #toolParams.lineManagerStateRef:get().lineListEntitiesSelected == 1 then
								-- switchLVMMode({"SEGMENT", -1})
							else
								switchLVMMode({"MAIN", nil})
							end
						end,
						isEnabledReturnTrue,
						_("Inspector")
					)
				)

				if lvmActionButtonsState:old() == "NONE" then
					react.useInputAction(
						commonParams.lvmButtonIAs.btn2D_ActionButtons,
						react.iaHandler(fnFocusActionButtons)
					)
				end
			end

			react.useInputAction("IA_NAV_LEFT", react.iaFocusTraversal(true, false))
			react.useInputAction("IA_NAV_RIGHT", react.iaFocusTraversal(true, true))
			react.useInputAction("IA_NAV_UP", react.iaFocusTraversal(false, false))
			react.useInputAction("IA_NAV_DOWN", react.iaFocusTraversal(false, true))
		elseif lvm2D3DModeState:old() == "3D" then
			controllerStyleClasses[#controllerStyleClasses + 1] = "manager3DMode"
			if #toolParams.lineManagerStateRef:get().lineListEntitiesSelected == 0 then
				controllerStyleClasses[#controllerStyleClasses + 1] = "none-selected"
			end

			react.useInputAction( -- 1/4 way to exit 3D
				"IA_MENU_BACK",
				react.iaHandler(
					function()
						lvm2D3DModeState:set("2D")
						creatingNewLineState:set(false)
						switchLVMMode({"MAIN", nil})
						lvmExit3DControllerFocusRef:set(2)
					end,
					isEnabledReturnTrue,
					_("Line Manager")
				)
			)
			react.useInputAction( -- 2/4 way to exit 3D
				commonParams.lvmButtonIAs.btn2D3D_Switch,
				react.iaHandler(
					function()
						lvm2D3DModeState:set("2D")
						creatingNewLineState:set(false)
						if #toolParams.lineManagerStateRef:get().lineListEntitiesSelected == 1 then
							-- switchLVMMode({"SEGMENT", -1})
						else
							switchLVMMode({"MAIN", nil})
						end
						lvmExit3DControllerFocusRef:set(2)
					end
				)
			)
		end
	end

	if gamepadInputModeState:old() then
		react.setStyleClasses(table.unpack(controllerStyleClasses))
	else
		react.setStyleClasses("managerMouseMode")
	end

	local preferredInitialSelectionRowKey : ReactStateT<Engine.Entity> = react.useState()

	react.onStep(function()
		if gamepadInputModeState:old() then
			if lvmInitialControllerFocusRef:get() and btnNewLineRef:get() then
				btnNewLineRef:get():focus()
				lvmInitialControllerFocusRef:set(false)
			end

			if lvmExit3DControllerFocusRef:get() > 0 then
				if not gamepadInputModeState:old() then
					lvmExit3DControllerFocusRef:set(0)
					return
				else
					lvmExit3DControllerFocusRef:set(lvmExit3DControllerFocusRef:get() - 1)
				end

				if lvmExit3DControllerFocusRef:get() == 0 then
					local anyVehicleSelected = #params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesSelected > 0 or
							#params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesUnselected > 0
					local anyLineOrDepotSelected = #params.toolParam.lineManagerStateRef:get().lineListEntitiesSelected > 0 or #params.toolParam.lineManagerStateRef:get().depotListEntitiesSelected > 0

					if anyLineOrDepotSelected then
						fnToLP()
					elseif anyVehicleSelected then
						fnToVMNext()
					end
				end
			end

			local anyVehicleSelected = #params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesSelected > 0 or
					#params.toolParam.vehicleManagerStateRef:get().vehicleListEntitiesUnselected > 0
			local anyLineSelected = #params.toolParam.lineManagerStateRef:get().lineListEntitiesSelected > 0
			local anyDepotSelected = #params.toolParam.lineManagerStateRef:get().depotListEntitiesSelected > 0

			if anyVehicleSelected or anyLineSelected or anyDepotSelected then
				gamepadNo3DSelectionInfoState:set(false)
			else
				gamepadNo3DSelectionInfoState:set(true)
			end
		elseif gamepadNo3DSelectionInfoState:old() then
			gamepadNo3DSelectionInfoState:set(false)
		end

		if vehicleStoreEventParamState:old()
			and vehicleStoreOpenForDepotAndLineRef
			and vehicleStoreOpenForDepotAndLineRef:get() then

			local vehicleStoreMode = vehicleStoreOpenForDepotAndLineRef:get().vehicleStoreMode

			if vehicleStoreMode == "Buy" then
				react.fireEvent(nil, "buyVehicles", vehicleStoreEventParamState:old())
			elseif vehicleStoreMode == "Modify" then
				react.fireEvent(nil, "modifyVehicles", vehicleStoreEventParamState:old())
			elseif vehicleStoreMode == "Replace" then
				react.fireEvent(nil, "replaceVehicles", vehicleStoreEventParamState:old())
			end

			-- local vehicleDepotDetails : LineUtil.VehicleDepotDetails = {
			-- 	entity = depotEntity,
			-- 	removeFromList = false,
			-- }
			-- commonParams.selectOrDeselectDepot(vehicleDepotDetails)
			-- commonParams.addOrRemoveDepotVehiclesToVehicleList(vehicleDepotDetails)

			vehicleStoreEventParamState:set(nil)
		end

		local prevData = bufferedSendDataRef:get()
		if #prevData > 0 then
			local newData : {BufferedLVMSendData} = {}
			if #toolParams.lineManagerStateRef:get().lineListEntitiesSelected == 1 then
				local singleLine : Engine.Entity = toolParams.lineManagerStateRef:get().lineListEntitiesSelected[1].entity
				for __, data in ipairs(prevData) do
					if api.engine.entityExists(data.vehicle)
						and api.engine.entityExists(data.line) then

						if data.line == singleLine then
							table.insert(newData, data)
						end
					end
				end
			end
			bufferedSendDataRef:set(newData)
		end

		local depotsSelected = toolParams.lineManagerStateRef:get().depotListEntitiesSelected
		if depotsSelected and #depotsSelected > 0 then
			local depot2vehicles = depot2VehiclesCompareRef:get()
			for __, depotSelectedEAR in ipairs(depotsSelected) do
				local depotVehicles = api.engine.system.transportVehicleSystem.getDepotVehicles(depotSelectedEAR.entity)

				for __, vehicle in ipairs(depotVehicles) do
					local depotVehiclesCompare = depot2vehicles[depotSelectedEAR.entity]
					if depotVehiclesCompare then
						if not table_util.arrayContains(depotVehiclesCompare, vehicle) then
							commonParams.addOrRemoveVehicleToVehicleList({
								entity = vehicle,
								removeFromList = false,
							})
							commonParams.addVehicleToDepotVehicleCompare(
								vehicle,
								depotSelectedEAR.entity
							)
						end
					end
				end
			end
		end

		if feedbackViewerRef:get() then
			hasFeedbackState:set(feedbackViewerRef:get():getApi().hasFeedback())
		end

		if openedWithSendToLineRef:get() then
			openedWithSendToLineRef:set(false)

			local vehicleSelection : {Engine.Entity} = {}
			if toolParams.openWithVehicleEntities then
				vehicleSelection = toolParams.openWithVehicleEntities
			else
				vehicleSelection = getSelectedVehicleEntities(toolParams.vehicleManagerStateRef)
			end

			local function getWindowApi() : Builtin.WindowAPI
				return toolParams.gameCtx.windowContainer:get():getApi()
			end

			local wc = getWindowApi()

			local pos = sendToLinePopoverPosRef:get():getPosition(1.0, 1.0)
			wc.removeAllWindows(popover_react_util.PopoverWindow)
			wc.addWindow(popover_react_util.PopoverWindow, "send-to-line", {
				onClose = function()
					cleanupSendToLine(
						toolParams.gameCtx,
						sendToLineDisabledState,
						switchToPrevLVMMode
					)
				end,
				x = pos.x,
				y = pos.y,
				windowTitle = _("Send To Line"),
				windowClass = "select-line, management",
				recipe = SendToLineSelection as IRecipe,
				params = {
					commonParams = commonParams,
				    selection = vehicleSelection,
				    sendToLine = function(lineEntity : Engine.Entity, sendScheme : FirstStopToSend.Scheme)
					sendVehiclesToLineCommands(
						vehicleSelection,
						lineEntity,
							sendScheme,
							afterSendingToLineFixup,
							bufferedSendDataRef,
							false -- doNotBuffer
					)
						cleanupSendToLine(
							toolParams.gameCtx,
							sendToLineDisabledState,
							switchToPrevLVMMode
						)
				    end,
					onlyVisible = false,
				    firstStopToSendSchemesRef = toolParams.firstStopToSendSchemesRef,
				    firstStopToSendSchemeMapRef = toolParams.firstStopToSendSchemeMapRef,
				},
				inputActionBubbleUpFilter = builtin.type.InputActionBubbleUpFilter.filterOut{
					"IA_CAMPAIGN_MISSION_MENU_OPEN",
				}
			})

			switchLVMMode({"SEND_TO_LINE", nil})
		end

		-- Note: this has to happen after the previous check, otherwise the popover gets created too soon
		if toolParams.needsToUpdateThenConsumeUpdate() then
			switchLVMMode(toolParams.storedLVMModeRef:get())
			if toolParams.sendToLineMode or (toolParams.storedLVMModeRef:get()[1] == "SEND_TO_LINE") then
				openedWithSendToLineRef:set(true)
			end

			local openLine = toolParams.openWithLineEntity
			local openVehicles = toolParams.openWithVehicleEntities
			local openDepot = toolParams.openWithDepotEntity

			if openLine then
				local ladlApi : LineAndDepotListApi = nil
				if lineAndDepotListRef:get() then
					ladlApi = lineAndDepotListRef:get():getApi()
				end

				if ladlApi then
					ladlApi.deselectAllLinesAndDepots()
					ladlApi.selectLine(openLine, true, nil)
				end
				local reactLine = line_util.getReactLineFromGameState(openLine)
				local insertAt = #reactLine.path
				switchLVMMode({"SEGMENT", insertAt})
				if gamepadInputModeState:old() then
					api.gui.camera.followEntity(openLine, true)
				end
				commonParams.addOrRemoveLineVehiclesToVehicleList(openLine, false, true)
				preferredInitialSelectionRowKey:set(openLine)
			end
			if openVehicles then
				local vmApi : VehicleManagerApi = nil
				if vehicleManagerRef:get() then
					vmApi = vehicleManagerRef:get():getApi()
				end

				if vmApi then
					vmApi.clearVehicleList()
					vmApi.addVehiclesToVehicleListAndSelect(openVehicles)
				end
			end
			if openDepot then
				if commonParams.areAnyVehiclesInDepot(openDepot) then
					local vehicleDepotDetails = {
						entity = openDepot,
						removeFromList = false -- Note: always select depot and vehicles when opening from entity window, never deselect
					}
					commonParams.selectOrDeselectDepot(vehicleDepotDetails)
					commonParams.addOrRemoveDepotVehiclesToVehicleList(vehicleDepotDetails)
				else
					commonParams.openVehicleDepot(openDepot, nil)
				end
				preferredInitialSelectionRowKey:set(openDepot)
			end
		end
	end)

	local managerChildren = {
		builtin.Component{
			layout = builtin.FloatingLayout{
				meta = { class = "keyhints" },
				children = {
					builtin.FloatingLayoutChild{
						h = -1,
						v = -1,
						item = LineVehicleManagerTopBar{
							commonParams = commonParams,
							filterState = localLVMFilterState,
						},
					},
					-- lvmControllerModeState:old().mode == "3D" and builtin.FloatingLayoutChild{
					-- 	overflowMode = builtin.type.FloatingLayoutOverflowMode.Overflow,
					-- 	h = 1,
					-- 	v = 0.5,
					-- 	item = builtin.KeybindingHintDisplay {
					-- 		meta = { class = "overflow-mode" },
					-- 		source = selfRef,
					-- 		inputAction = commonParams.lvmButtonIAs.btn2D3D_Switch,
					-- 	},
					-- } or nil,
				},
			},
		},
		builtin.Component{
			layout = builtin.FloatingLayout{
				children = {
					builtin.FloatingLayoutChild{
						item = LineManager{
							meta = {
								tag = "menu.management.line",
								onFocusChange = function(focused : boolean)
									if focused then
										lvmSectionState:set("LM")
									end
								end,
							},

							lineManagerActionButtonsRef = lineManagerActionButtonsRef,
							scrollToNewLineRef = scrollToNewLineRef,

							filterState = localLVMFilterState,
							filterConfigState = lvmFilterConfigState,
							preferredInitialSelectionRowKey = preferredInitialSelectionRowKey,

							commonParams = commonParams,
						},
					},
					builtin.FloatingLayoutChild{
						h = -1,
						v = -1,
						item = builtin.Component{
							meta = {
								class = "line-manager, greyed-out",
								mouseTransparent = true,
							},
						},
					},
				},
			},
		},
		builtin.Component{
			layout = builtin.FloatingLayout{
				children = {
					builtin.FloatingLayoutChild{
						item = VehicleManager(react.ref(vehicleManagerRef), {
							meta = {
								onFocusChange = function(focused : boolean)
									if focused then
										lvmSectionState:set("VM")
									end
								end,
							},

							vehicleManagerActionButtonsRef = vehicleManagerActionButtonsRef,
							vehicleListRef = vehicleListRef,
							firstStopToSendSchemesRef = toolParams.firstStopToSendSchemesRef,
							firstStopToSendSchemeMapRef = toolParams.firstStopToSendSchemeMapRef,

							afterSendingToLineFixup = afterSendingToLineFixup,
							scrollToRow = function(rowEntity : Engine.Entity)
								local ladlApi : LineAndDepotListApi = nil
								if lineAndDepotListRef:get() then
									ladlApi = lineAndDepotListRef:get():getApi()
								end

								if ladlApi then
									local forceTopAlign = true
									ladlApi.scrollToRow(rowEntity, forceTopAlign)
								end
							end,

							sendToLineDisabledState = sendToLineDisabledState,
							vehicleStoreEventParamState = vehicleStoreEventParamState,

							commonParams = commonParams,
						}),
					},
					builtin.FloatingLayoutChild{
						h = -1,
						v = 0,
						item = builtin.Component{
							meta = {
								class = "vehicle-manager, greyed-out",
								mouseTransparent = true,
							},
						},
					},
					gamepadInputModeState:old() and builtin.FloatingLayoutChild{
						h = 1,
						v = 0.5,
						overflowMode = builtin.type.FloatingLayoutOverflowMode.Overflow,
						item = builtin.Component{
							meta = {
								class = "arrow-to-hud, overflow-mode",
								mouseTransparent = true,
							},
							layout = builtin.FloatingLayout{
								children = {
									builtin.FloatingLayoutChild{
										h = 0,
										v = 0,
										item = builtin.ImageView{
											meta = {
												class = "arrow-1",
												mouseTransparent = true,
											},
											path = "::/gui/main/icons/symbol_resume_42.tga",
											scaling = builtin.type.ImageViewScaling.AutoFit,
										},
									},
									builtin.FloatingLayoutChild{
										h = 0,
										v = 0,
										item = builtin.ImageView{
											meta = {
												class = "arrow-2",
												mouseTransparent = true,
											},
											path = "::/gui/main/icons/symbol_resume_42.tga",
											scaling = builtin.type.ImageViewScaling.AutoFit,
										},
									},
								},
							},
						},
					} or nil,
				},
			},
		},
		builtin.FloatingLayout{
			children = {
				builtin.FloatingLayoutChild{
					item = LinePanel{
						meta = {
							onFocusChange = function(focused : boolean)
								if focused then
									lvmSectionState:set("LP")
								end
							end,
						},

						commonParams = commonParams,

						linePanelRef = linePanelRef,
						linePanelActionButtonsRef = linePanelActionButtonsRef,
						autoRenameSchemeComponentsMapRef = toolParams.autoRenameSchemeComponentsMapRef,
						autoRenameSchemesRef = toolParams.autoRenameSchemesRef,
						autoRenameSchemeRef = toolParams.autoRenameSchemeRef,
						firstStopToSendSchemesRef = toolParams.firstStopToSendSchemesRef,
						firstStopToSendSchemeMapRef = toolParams.firstStopToSendSchemeMapRef,
					},
				},
				builtin.FloatingLayoutChild{
					h = -1,
					v = 0,
					item = builtin.Component{
						meta = {
							class = "line-panel, greyed-out",
							mouseTransparent = true,
						},
					},
				},
				builtin.FloatingLayoutChild{
					h = 0.5,
					v = 0,
					item = builtin.Component{
						meta = {
							class = hasFeedbackState:old() and "feedback-container" or "",
						},
						layout = builtin.BoxLayout{
							child = feedback_list_util.FeedbackList(react.ref(feedbackViewerRef), {}),
						},
					}
				},
			}
		},
	}

	return builtin.BoxLayout{
		children = {
			builtin.Component{
				meta = {
					disableFocusableRecursive = gamepadInputModeState:old() and lvm2D3DModeState:old() == "3D",
				},
				layout = builtin.BoxLayout{
					children = managerChildren
				}
			}
		}
	}
end)

local function getWindowApi(gameCtx : GameContext) : Builtin.WindowAPI
	return gameCtx.windowContainer:get():getApi()
end

local managerToolWindowId = "menu.management"

manager_window.ManagerToolWindow = react.RegisterWrapperRecipe("ManagerWindow", builtin.Window, function(param : ToolReactUtil.IToolWindowParam<ManagerWindow.ManagerToolParam>) : TreeNodeId
	return builtin.Window{
		id = managerToolWindowId,
		compact = true,
		closable = true,
		onClose = function()
			param.onClose()
		end,
		content = manager_window.ManagerWindowContent(param.payload),
		autoFocusOnBecomingVisible = false,
		autoVisibilityOnFocusChange = false,
		tool = "management",
		inputActionBubbleUpFilter = builtin.type.InputActionBubbleUpFilter.allowOnly{
			"IA_MANAGEMENT_MODE",

			"IA_MENU",
			"IA_MENU_BACK",
			"IA_GAME_PAUSE_OR_CYCLE_SPEED",
			"gamePause",
			"gameCycleSpeed",
			"IA_CONTEXT_HELP",
			"IA_GUIDE",
			"IA_CLOSE_TOPMOST_WINDOW",
			"IA_SKIP_VOICEOVER_AND_CUTSCENE",

			"IA_ACCEPT",
			"IA_SELECT",
			"IA_APPLY",

			"IA_UP",
			"IA_DOWN",
			"IA_LEFT",
			"IA_RIGHT",
			"IA_NAV_UP",
			"IA_NAV_DOWN",
			"IA_NAV_LEFT",
			"IA_NAV_RIGHT",

			"IA_OPTION1",

			"IA_CAMERA_PAN",
			"IA_CAMERA_ROTATE_X",
			"IA_CAMERA_TILT",
			"IA_CAMERA_ZOOM_IN",
			"IA_CAMERA_ZOOM_OUT",

			"cameraMoveUp",
			"cameraMoveDown",
			"cameraMoveLeft",
			"cameraMoveRight",
			"cameraRotateLeft",
			"cameraRotateRight",
			"cameraTiltUp",
			"cameraTiltDown",
			"cameraZoomIn",
			"cameraZoomOut",

			"IA_GAME_DEBUG_TOGGLE_DEBUG_WINDOW",
			"IA_DEBUG_OUTPUT_COMPONENT_UNDER_CURSOR",
			"IA_DEBUG_OUTUPUT_FOCUSED_COMPONENT",
			"IA_DEBUG_RELOAD_REACT_COMPONENT",
			"IA_DEBUG_RELOAD_DEFINITIONS",
			"IA_DEBUG_RELOAD_SHADERS",
			"IA_DEBUG_RELOAD_STATE_PROVIDER",
			"uiCloseAll",
		}
	}
end)

local makeToolWindowParam = function(toolParam : ManagerWindow.ManagerToolParam, ctx : Builtin.ToolStackContext<ManagerWindow.ManagerToolParam>) : ToolReactUtil.IToolWindowParam<ManagerWindow.ManagerToolParam>
	local result : ToolReactUtil.IToolWindowParam<ManagerWindow.ManagerToolParam> = {}

	result.onClose = function()
		log.warning("will popSelf")

		local wc = toolParam.gameCtx.windowContainer:get():getApi()
		wc.removeAllWindows(popover_react_util.PopoverWindow)
		wc.removeAllWindows(CargoFilterWindow)

		ctx.popSelf()
	end

	result.payload = {}
	result.payload.setActionFn = ctx.setActionFn
	result.payload.toolParam = toolParam

	return result
end

local function addOrUpdateWindow(ctx : Builtin.ToolStackContext<ManagerWindow.ManagerToolParam>, toolParam : ManagerWindow.ManagerToolParam)
	local windowApi = getWindowApi(toolParam.gameCtx)
	local windowParams : ToolReactUtil.IToolWindowParam<ManagerWindow.ManagerToolParam> = makeToolWindowParam(toolParam, ctx)
	windowApi.addSingletonWindow(manager_window.ManagerToolWindow, windowParams)

	-- in case the LVM has been shelved but is now actively reopened
	api.gui.byId.setVisible(managerToolWindowId, true)
	getWindowApi(toolParam.gameCtx).moveSingletonWindowToFront(manager_window.ManagerToolWindow)
end

manager_window.ManagerTool = react.RegisterTool{
	name = "Manager",
	push = function(ctx : Builtin.ToolStackContext<ManagerWindow.ManagerToolParam>, toolParam : ManagerWindow.ManagerToolParam)
		log.verbose("Push management mode")
		addOrUpdateWindow(ctx, toolParam)
	end,

	pop = function(__ctx : Builtin.ToolStackContext<ManagerWindow.ManagerToolParam>, toolParam : ManagerWindow.ManagerToolParam)
		log.verbose("Quit management mode")
		local windowApi = getWindowApi(toolParam.gameCtx)
		windowApi.removeAllWindows(vehicle_store_window.VehicleStoreWindow)
		windowApi.removeAllWindows(manager_window.ManagerToolWindow)
	end,

	shelve = function(__ctx : Builtin.ToolStackContext<ManagerWindow.ManagerToolParam>, toolParam : ManagerWindow.ManagerToolParam, shelve : boolean)
		if shelve then
			api.gui.byId.setVisible(managerToolWindowId, false)
		else
			api.gui.byId.setVisible(managerToolWindowId, true)
			getWindowApi(toolParam.gameCtx).moveSingletonWindowToFront(manager_window.ManagerToolWindow)
		end
	end,
}

manager_window.ManagerEntryPoint = react.RegisterRecipe("ManagerEntryPoint", function(entryPointParam : ManagerWindow.ManagerEntryPointParam) : TreeNodeId
	local vehicleManagerStateRef : ReactRefT<VehicleManagerState> = react.useRef({
		vehicleListEntitiesSelected = {},
		vehicleListEntitiesUnselected = {},
		carriers = {},
	})
	local lineManagerStateRef : ReactRefT<LineManagerState> = react.useRef({
		lineListEntitiesSelected = {},
		lineListEntitiesUnselected = {},
		depotListEntitiesSelected = {},
		depotListEntitiesUnselected = {},
	})
	local lvmFilterStateRef : ReactRefT<StatisticsReactUtil.FilterState> = react.useRef({
		category = {},
		search = "",
		onlyVisible = false
	})
	local storedLVMModeRef : ReactRefT<{LVMMode, any}> = react.useRef({"MAIN", nil})

	local autoRenameSchemeComponentsMapRef : ReactRefT<{string : AutoRename.SchemeComponent}> = react.useRefLazy(function() : {string : AutoRename.SchemeComponent}
		local autoRenameSchemeComponentsMap : {string : AutoRename.SchemeComponent} = {}

		local allSchemeComponentRes = api.res.genericRep.getAllOfType("rename_scheme_component")
		for i = 1, #allSchemeComponentRes do
			local schemeComponentRes = api.res.genericRep.get(allSchemeComponentRes[i] as integer)
			local schemeComponentData = schemeComponentRes.data as AutoRename.SchemeComponent

			autoRenameSchemeComponentsMap[schemeComponentData.schemeComponentNameRaw] = schemeComponentData
		end

		return autoRenameSchemeComponentsMap
	end)
	local autoRenameSchemesRef : ReactRefT<{AutoRename.Scheme}> = react.useRefLazy(function() : {AutoRename.Scheme}
		local autoRenameSchemes: {AutoRename.Scheme} = {}

		local allSchemeRes = api.res.genericRep.getAllOfType("rename_scheme")
		for i = 1, #allSchemeRes do
			local schemeRes = api.res.genericRep.get(allSchemeRes[i] as integer)
			local schemeData = schemeRes.data as AutoRename.Scheme

			table.insert(autoRenameSchemes, schemeData);
		end

		table.sort(autoRenameSchemes, function(a : AutoRename.Scheme, b : AutoRename.Scheme) : boolean
			if a.sortKey == b.sortKey then
				return a.schemeNameRaw < b. schemeNameRaw
			end
			return a.sortKey < b.sortKey
		end)

		return autoRenameSchemes
	end)

	local autoRenameSchemeRef : ReactRefT<AutoRename.Scheme> = react.useRef(autoRenameSchemesRef:get()[1])

	local firstStopToSendSchemesRef : ReactRefT<{FirstStopToSend.Scheme}> = react.useRefLazy(function() : {FirstStopToSend.Scheme}
		local firstStopToSendSchemes: {FirstStopToSend.Scheme} = {}

		local allSchemeRes = api.res.genericRep.getAllOfType("firstStopToSend_scheme")
		for i = 1, #allSchemeRes do
			local schemeRes = api.res.genericRep.get(allSchemeRes[i] as integer)
			local schemeData = schemeRes.data as FirstStopToSend.Scheme

			table.insert(firstStopToSendSchemes, schemeData);
		end

		table.sort(firstStopToSendSchemes, function(a : FirstStopToSend.Scheme, b : FirstStopToSend.Scheme) : boolean
			if a.sortKey == b.sortKey then
				return a.schemeNameRaw < b. schemeNameRaw
			end
			return a.sortKey < b.sortKey
		end)

		return firstStopToSendSchemes
	end)

	-- vanilla available schemes: "evenlydistributed", "firstcargocompatiblestop", "firststop", "nextreachable"
	local firstStopToSendSchemeMapRef : ReactRefT<{Carrier : FirstStopToSend.Schemes}> = react.useRef({ -- value : FirstStoptoSend.Scheme.schemeNameRaw
		[api.type["enum"].Carrier.ROAD] = {
			supportsPassengers = "evenlydistributed",
			cargoOnly          = "firststop",
		},
		[api.type["enum"].Carrier.RAIL] = {
			supportsPassengers = "nextreachable",
			cargoOnly          = "nextreachable",
		},
		[api.type["enum"].Carrier.TRAM] = {
			supportsPassengers = "nextreachable",
			cargoOnly          = "nextreachable",
		},
		[api.type["enum"].Carrier.AIR] = {
			supportsPassengers = "evenlydistributed",
			cargoOnly          = "firststop",
		},
		[api.type["enum"].Carrier.WATER] = {
			supportsPassengers = "evenlydistributed",
			cargoOnly          = "firststop",
		},
	})

	local pushUpdate : ReactRefT<boolean> = react.useRef(false)
	local needsToUpdateThenConsumeUpdate = function() : boolean
		if pushUpdate:get() then
			pushUpdate:set(false)
			return true
		end
	end

	local rememberSelection = false

	react.onEvent("openVehicleManager", function(__ : string, param : ManagerWindow.ManagerWindowEventParam)
		pushUpdate:set(true)

		if rememberSelection then
			-- check if entities still exist since last time opening the LVM
			vehicleManagerStateRef:set(makeVehicleManagerState(
				vehicleManagerStateRef:get().vehicleListEntitiesSelected,
				vehicleManagerStateRef:get().vehicleListEntitiesUnselected
			))
			lineManagerStateRef:set(makeLineManagerState(
				lineManagerStateRef:get().lineListEntitiesSelected,
				lineManagerStateRef:get().depotListEntitiesSelected,
				nil
			))
			if storedLVMModeRef:get()[1] == "SEND_TO_LINE" then
				storedLVMModeRef:set({"MAIN", nil})
			end
		else
			vehicleManagerStateRef:set(makeVehicleManagerState({}, {}))
			lineManagerStateRef:set(makeLineManagerState({}, {}, nil))
			storedLVMModeRef:set({"MAIN", nil})
		end

		local wc = entryPointParam.gameCtx.windowContainer:get():getApi()
		wc.removeAllWindows(vehicle_store_window.VehicleStoreWindow)

		entryPointParam.gameCtx.toolStack:get():getApi().push(manager_window.ManagerTool, nil, {
			gameCtx = entryPointParam.gameCtx,
			openWithLineEntity = param.openWithLineEntity as Engine.Entity,
			openWithVehicleEntities = param.openWithVehicleEntities as {Engine.Entity},
			openWithDepotEntity = param.openWithDepotEntity as Engine.Entity,
			sendToLineMode = param.sendToLineMode as boolean,
			vehicleManagerStateRef = vehicleManagerStateRef,
			lineManagerStateRef = lineManagerStateRef,
			lvmFilterStateRef = lvmFilterStateRef,
			storedLVMModeRef = storedLVMModeRef,
			autoRenameSchemeComponentsMapRef = autoRenameSchemeComponentsMapRef,
			autoRenameSchemesRef = autoRenameSchemesRef,
			autoRenameSchemeRef = autoRenameSchemeRef,
			firstStopToSendSchemesRef = firstStopToSendSchemesRef,
			firstStopToSendSchemeMapRef = firstStopToSendSchemeMapRef,
			needsToUpdateThenConsumeUpdate = needsToUpdateThenConsumeUpdate,
		})
	end)
	react.onEvent("closeVehicleManager", function(__ : string, __param : any)
		entryPointParam.gameCtx.toolStack:get():getApi().pop(manager_window.ManagerTool, nil)
	end)
	react.onEvent("duplicateVehicles", function(__ : string, param : DuplicateVehiclesParam)
		log.verbose("Clone vehicle")

		local toAdd : {VehicleReactUtil.VehicleChange} = {}
		for __, vehicleEntity in ipairs(param.vehicleEntities) do
			local tv = api.engine.getComponent(vehicleEntity, api.type.ComponentType.TRANSPORT_VEHICLE) as Engine.Component.TransportVehicle
			local newConfig = api.type.TransportVehicleConfig.new(tv.transportVehicleConfig)
			local newVehicles : {TransportVehiclePart} = {}
			for __, part in ipairs(newConfig.vehicles) do
				newVehicles[#newVehicles + 1] = part
				part.purchaseTime = 0
				part.maintenanceState = 1
			end
			newConfig.vehicles = newVehicles
			local modelId2quantity : { integer : integer } = {}
			for __, vehicle in ipairs(newConfig.vehicles) do
				modelId2quantity[vehicle.part.modelId] = (modelId2quantity[vehicle.part.modelId] or 0) + 1
			end
			local missionError = api.gui.fireGuiScriptEvent("vehicleStore", "mission.buyVehicle", {
				modelId2quantity = modelId2quantity,
				actuallyBuy = true,
			}) as string
			if missionError then
				param.addFeedback(missionError, nil, nil, nil)
				break
			end
			api.gui.fireGuiScriptEvent("vehicleStore", "mission.cloneVehicle", {
				entity = vehicleEntity,
			})
			table.insert(toAdd, {
				vehicleEntity = -vehicleEntity - 1,
				config = newConfig,
			})
		end

		local getFirstStopToSendTo = function(
			vehicleToSendToLine : Engine.Entity,
			lineToSendTo : Engine.Entity
		) : integer
			local sendScheme = getSendScheme(
				{vehicleToSendToLine},
				firstStopToSendSchemesRef,
				firstStopToSendSchemeMapRef
			)

			local sendFn = util.useFn(sendScheme.firstStopToSendFnDefinition.fileName) as function(params : FirstStopToSend.SendParams) : {Engine.Entity : integer}

			local vehicle2stopIndex : {Engine.Entity : integer} = sendFn({
				vehicleEntities = {vehicleToSendToLine},
				lineEntity = lineToSendTo
			})

			return vehicle2stopIndex[vehicleToSendToLine]
		end

		vehicle_react_util.HandleVehicleChanges(
			toAdd,
			entryPointParam.gameCtx.filters:get().protectedEntities,
			function(vehicleEntity : Engine.Entity) : {Engine.Entity, Engine.Entity}
				return api.engine.util.vehicle.findBestLineAndDepotForVehicle(-vehicleEntity-1)
			end,
			getFirstStopToSendTo,
			param.onBuy,
			function(reason : string)
				param.addFeedback(reason, nil, nil, nil)
			end
		)
	end)
end)

return manager_window
