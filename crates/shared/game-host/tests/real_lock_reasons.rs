//! Locked production items, purchases and policies carry the real reason on a real game: the
//! core's own tooltip text for units and buildings, the failing check for policies.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Start};
use serde_json::Value;
#[path = "common/real.rs"]
mod common;
#[path = "common/city_screen.rs"]
mod screen_helper;

const GENERAL: &str = "Production requirements are not met in this city.";
const GENERAL_BUY: &str = "Insufficient funds or purchase requirements are not met.";

/// Runs a test-bridge chunk and returns the global `out` it sets (a string).
fn bridge(h: &mut common::H, code: &str) -> String {
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(code, "=lock-reasons-bridge").unwrap();
    lua.globals().get("out").unwrap()
}

/// The core text the screens show: markup removed as `city.lua` does.
fn markup(text: &str) -> String {
    let mut out = text.replace("[NEWLINE]", "\n").replace("[ENDCOLOR]", "");
    while let Some(start) = out.find("[COLOR") {
        let end = out[start..].find(']').map_or(out.len(), |e| start + e + 1);
        out.replace_range(start..end, "");
    }
    while let Some(start) = out.find("[ICON_") {
        let end = out[start..].find(']').map_or(out.len(), |e| start + e + 1);
        let name = out[start + 6..end - 1].to_owned();
        out.replace_range(start..end, &format!("{name} "));
    }
    out.trim().to_owned()
}

/// The same for the HUD (`hud.lua`): line breaks become spaces, icons are dropped.
fn hud_markup(text: &str) -> String {
    let mut out = text.replace("[NEWLINE]", " ").replace("[ENDCOLOR]", "");
    for tag in ["[COLOR", "[ICON_"] {
        while let Some(start) = out.find(tag) {
            let end = out[start..].find(']').map_or(out.len(), |e| start + e + 1);
            out.replace_range(start..end, "");
        }
    }
    out.trim().to_owned()
}

fn policy<'a>(root: &'a Value, key: &str) -> &'a Value {
    root["branches"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["policies"].as_array().unwrap())
        .find(|p| p["key"] == key)
        .unwrap_or_else(|| panic!("policy {key}"))
}

#[test]
fn locked_items_explain_themselves() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    h.found_city(0);
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;

    // A size-1 city cannot train a Settler: the core says why (its canTrain tooltip sink).
    let settler: i64 = bridge(&mut h, "out = tostring(GameInfoTypes.UNIT_SETTLER)")
        .parse()
        .unwrap();
    let core_text = bridge(
        &mut h,
        &format!(
            "local c = Players[0]:GetCityByID({city}); assert(not c:CanTrain({settler}, 0, 0)); out = c:CanTrainTooltip({settler})"
        ),
    );
    let expected = markup(&core_text);
    assert!(
        !expected.is_empty() && !expected.contains("TXT_KEY"),
        "{core_text:?}"
    );
    let root = screen_helper::query(&mut h, "city", city);
    let options: Vec<&Value> = root["production"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["options"].as_array().unwrap())
        .collect();
    let row = options
        .iter()
        .find(|o| o["item"] == settler)
        .expect("the Settler is listed");
    assert_eq!(row["can_build"], false);
    assert_eq!(row["reason"], expected.as_str(), "{row}");
    eprintln!("settler lock: {expected:?}");

    // Purchases: no gold at the start; the core's purchase tooltip names the shortage.
    let bought = options
        .iter()
        .find(|o| o["buy_reason"].is_string() && o["gold_state"] == "short")
        .expect("an unaffordable purchase");
    let reason = bought["buy_reason"].as_str().unwrap();
    assert_ne!(reason, GENERAL_BUY, "{bought}");
    let item = bought["item"].as_i64().unwrap();
    let (order, id) = (item / 65536, item % 65536);
    let getter = if order == 0 {
        "GetPurchaseUnitTooltip"
    } else {
        "GetPurchaseBuildingTooltip"
    };
    let core_buy = bridge(
        &mut h,
        &format!("out = Players[0]:GetCityByID({city}):{getter}({id})"),
    );
    assert_eq!(reason, markup(&core_buy));
    eprintln!("purchase lock: {reason:?}");

    // HUD city focus: the same core reason for the Settler.
    let hud = screen_helper::query_args(&mut h, "hud", vec![("city".into(), city.into())]);
    let locked = hud["city"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == settler)
        .expect("the Settler lock in the HUD");
    let hud_text = locked["reason"].as_str().unwrap();
    assert_ne!(hud_text, GENERAL);
    assert_eq!(hud_text, hud_markup(&core_text));

    // Policies: the first failing core check, in the core's order.
    let tradition = bridge(
        &mut h,
        r#"local b = GameInfo.PolicyBranchTypes.POLICY_BRANCH_TRADITION
           for p in GameInfo.Policies{PolicyBranchType = b.Type} do
             if p.Type ~= b.FreePolicy and p.Type ~= b.FreeFinishingPolicy then
               local n = 0
               for _ in GameInfo.Policy_PrereqPolicies{PolicyType = p.Type} do n = n + 1 end
               if n > 0 then out = p.Type; break end
             end
           end"#,
    );
    let root = screen_helper::query(&mut h, "policies", 0);
    let reason = policy(&root, &tradition)["reason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        reason.starts_with("Requires ") && reason.ends_with(" more culture"),
        "{reason}"
    );
    bridge(&mut h, "Players[0]:ChangeJONSCulture(100000); out = ''");
    let root = screen_helper::query(&mut h, "policies", 0);
    let reason = policy(&root, &tradition)["reason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        reason.starts_with("Open the ") && reason.ends_with(" branch first"),
        "{reason}"
    );
    let branch: i64 = bridge(
        &mut h,
        "out = tostring(GameInfo.PolicyBranchTypes.POLICY_BRANCH_TRADITION.ID)",
    )
    .parse()
    .unwrap();
    let opened = h.cmd(
        0,
        Command::ScreenAction {
            screen: "policies".into(),
            action: "unlock_branch".into(),
            args: vec![("a".into(), branch)],
            text: None,
        },
    );
    assert!(
        matches!(
            opened,
            CommandOutcome::Applied | CommandOutcome::ScreenApplied { .. }
        ),
        "{opened:?}"
    );
    bridge(&mut h, "Players[0]:ChangeJONSCulture(100000); out = ''");
    let root = screen_helper::query(&mut h, "policies", 0);
    let row = policy(&root, &tradition);
    // The prerequisites the seat does not hold yet, by the names the screen itself shows.
    let missing: Vec<&str> = row["requires"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| {
            root["branches"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|b| b["policies"].as_array().unwrap())
                .any(|p| p["id"] == r["id"] && p["state"] != "adopted")
        })
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(!missing.is_empty(), "{row}");
    assert_eq!(
        row["reason"],
        format!("Requires {}", missing.join(", ")).as_str()
    );
    eprintln!("policy lock: {}", row["reason"]);
}
