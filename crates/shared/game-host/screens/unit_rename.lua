return {
    data = function(player, args)
        if args.a == nil then return {unit = -1, name = "No unit selected", enabled = false, reason = "Choose an owned unit first."} end
        local unit = args.a and Players[player]:GetUnitByID(args.a)
        assert(unit and unit:GetOwner() == player, "Choose an owned unit first.")
        local enabled = Players[player]:IsTurnActive() and not unit:IsBusy() and not unit:IsDelayedDeath()
        return {unit = unit:GetID(), name = unit:GetName(), enabled = enabled,
            reason = not enabled and "The unit cannot be renamed now." or nil}
    end,
    actions = {
        rename = function(player, args, net)
            __screens.hud.actions.rename(player, args, net)
            return {close = true}
        end,
    },
}
