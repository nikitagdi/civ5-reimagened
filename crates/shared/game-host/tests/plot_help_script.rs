//! `plot_help`: what the seat may know of a plot (fog rules of its team) and the combat preview of its unit.
use lua_sys::{Lua, Table};

#[test]
fn plot_help_follows_the_seat_fog_and_previews_combat() {
    let lua = Lua::new().unwrap();
    lua.exec(include_str!("../screens/_lib.lua"), "=helpers")
        .unwrap();
    let screen: Table = lua
        .load(include_str!("../screens/plot_help.lua"))
        .eval()
        .unwrap();
    lua.globals().raw_set("screen", screen).unwrap();
    lua.exec(
        r#"
      function L(s) return s end
      GameDefines = {MAX_HIT_POINTS = 100}
      DomainTypes = {DOMAIN_SEA = 0, DOMAIN_LAND = 2}
      GameInfoTypes = {TECH_MINING = 3}
      GameInfo = {
        Terrains = {[0] = {Description = 'Grassland'}},
        Features = {[1] = {Type = 'FEATURE_FOREST', Description = 'Forest'}},
        Resources = {[2] = {Type = 'RESOURCE_IRON', Description = 'Iron', TechCityTrade = 'TECH_MINING',
          Help = 'Used by [ICON_PRODUCTION] units.[NEWLINE]Strategic.', Happiness = 0}},
        Resource_YieldChanges = function(cond)
          local rows = {}
          if cond == "ResourceType = 'RESOURCE_IRON'" then rows = {{YieldType = 'YIELD_PRODUCTION', Yield = 1}} end
          local i = 0
          return function() i = i + 1; return rows[i] end
        end,
        Technologies = {[3] = {Description = 'Mining'}},
        Improvements = {[4] = {Description = 'Mine'}},
        Routes = {[0] = {Description = 'Road'}},
        Yields = {[0] = {Description = 'Food'}, [1] = {Description = 'Production'}, [2] = {Description = 'Gold'},
          [3] = {Description = 'Science'}, [4] = {Description = 'Culture'}, [5] = {Description = 'Faith'}},
      }
      GameInfo.Yields.YIELD_PRODUCTION = GameInfo.Yields[1]
      Game = {GetCombatPrediction = function() return 5 end}
      Teams = {}
      for t = 0, 2 do
        local team = t
        Teams[t] = {IsAtWar = function(_, other) return (team == 0 and other == 1) or (team == 1 and other == 0) end,
          GetTeamTechs = function() return {HasTech = function() return false end} end,
          IsHasMet = function() return true end}
      end
      local function unit(owner, id, strength, ranged, damage)
        return {GetOwner = function() return owner end, GetTeam = function() return owner end, GetID = function() return id end,
          GetName = function() return 'Unit' .. id end, GetBaseCombatStrength = function() return strength end,
          GetBaseRangedCombatStrength = function() return ranged end, GetDamage = function() return damage end,
          IsInvisible = function() return false end, IsEmbarked = function() return false end,
          GetPlot = function() return nil end, GetDomainType = function() return 2 end, IsRangedSupportFire = function() return false end,
          GetMaxAttackStrength = function() return 1600 end, GetMaxDefenseStrength = function() return 1400 end,
          GetCombatDamage = function(self, mine, theirs) if self:GetOwner() == 0 then return 38 else return 24 end end}
      end
      local swordsman = unit(0, 7, 16, 0, 0)
      local spearman = unit(1, 9, 11, 0, 30)
      Players = {
        [0] = {GetTeam = function() return 0 end, GetUnitByID = function(_, id) if id == 7 then return swordsman end end,
          IsHuman = function() return true end, GetNickName = function() return '' end,
          GetCivilizationShortDescription = function() return 'Rome' end},
        [1] = {GetTeam = function() return 1 end, IsHuman = function() return false end, GetNickName = function() return '' end,
          GetCivilizationShortDescription = function() return 'Germany' end},
      }
      -- (1, 1): seen, forest hills with iron, a mine and a road, German, a German spearman.
      -- (2, 1): revealed under fog, same contents. (3, 1): never revealed.
      local function plot(x, revealed, seen)
        return {IsRevealed = function() return revealed end, IsVisible = function() return seen end,
          GetFeatureType = function() return 1 end, IsMountain = function() return false end, IsLake = function() return false end,
          GetTerrainType = function() return 0 end, IsHills = function() return true end, IsRiver = function() return false end,
          GetResourceType = function() return 2 end, GetNumResource = function() return 2 end,
          GetRevealedImprovementType = function() return 4 end, IsImprovementPillaged = function() return true end,
          GetRevealedRouteType = function() return 0 end, IsRoutePillaged = function() return false end,
          GetRevealedOwner = function() return 1 end, DefenseModifier = function() return 50 end, IsCity = function() return false end,
          CalculateYield = function(_, i) return ({[0] = 1, [1] = 3})[i] or 0 end,
          GetNumUnits = function() return 1 end, GetUnit = function() return spearman end}
      end
      local plots = {[1] = plot(1, true, true), [2] = plot(2, true, false), [3] = plot(3, false, false)}
      Map = {GetPlot = function(x, y) if y == 1 then return plots[x] end end}

      local seen = screen.data(0, {x = 1, y = 1, unit = 7})
      assert(seen.revealed and seen.visible and seen.cell == 65537)
      -- As GetNatureString: a feature that is not special keeps the terrain under it.
      assert(seen.nature == 'Forest, Grassland, TXT_KEY_PLOTROLL_HILL', seen.nature)
      assert(seen.resource == 'Iron' and seen.resource_count == 2 and seen.resource_tech == 'Mining')
      -- The original's resource tooltip: the quantity and the name in capitals, the help without its markup, the
      -- yields of the improved and worked resource (no happiness line for a strategic one).
      assert(seen.resource_title == '2 IRON', seen.resource_title)
      local lines = {}
      for i, l in ipairs(seen.resource_lines) do assert(l.idx == i - 1); lines[i] = l.text end
      assert(table.concat(lines, '|') == '• Used by units. Strategic.|TXT_KEY_RESOURCE_TOOLTIP_IMPROVED_WORKED|• +1 Production',
        table.concat(lines, '|'))
      assert(seen.improvement == 'Mine' and seen.improvement_pillaged and seen.route == 'Road')
      assert(seen.owner == 'Germany' and seen.owner_relation == 'war' and seen.defense == 50)
      assert(#seen.yields == 2 and seen.yields[1].key == 'food' and seen.yields[2].value == 3)
      assert(#seen.units == 1 and seen.units[1].relation == 'war' and seen.units[1].hp == 70)
      local c = seen.combat
      -- COMBAT_PREDICTION_SMALL_VICTORY is 5 in CombatPredictionTypes.
      assert(c and not c.ranged and c.outcome == 'small_victory', c and c.outcome)
      assert(c.my_strength == 16 and c.their_strength == 14)
      -- The damage each side takes: the swordsman deals 38, the spearman 24.
      assert(c.my_damage == 24 and c.their_damage == 38 and c.their_hp == 70, c.my_damage .. ' ' .. c.their_damage)

      -- Under the fog: what the team revealed, never the units, the yields or a pillage it cannot see.
      local fog = screen.data(0, {x = 2, y = 1, unit = 7})
      assert(fog.revealed and not fog.visible and fog.improvement == 'Mine' and not fog.improvement_pillaged)
      assert(#fog.units == 0 and #fog.yields == 0 and fog.combat == nil)

      -- Never revealed: nothing but the plot itself.
      local dark = screen.data(0, {x = 3, y = 1})
      assert(not dark.revealed and dark.nature == '' and dark.resource == nil and dark.owner == nil and #dark.units == 0)
      assert(dark.resource_title == nil and #dark.resource_lines == 0)

      -- Another seat's unit id gives no preview; a missing plot is an empty answer.
      local other = screen.data(0, {x = 1, y = 1, unit = 9})
      assert(other.combat == nil and #other.units == 1)
      assert(not screen.data(0, {x = 9, y = 9}).revealed)
      assert(not pcall(screen.data, 0, {x = 1.5, y = 1}))
    "#,
        "=plot_help",
    )
    .unwrap();
}
