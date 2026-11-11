//! `production.choose`, `replace`, `push`, `pop`, `move`, `buy`: the build queue of a city and buying in it.
//! The options and the queue come from the model of the production window; an intent for a city whose window
//! the model does not show cannot be checked and is refused.

use protocol::Command;
use ui2_model::generated::{BuyWith, Opt, Production, PushAt, RefuseWhy};
use ui2_model::target::{city_target, option_target};
use ui2_model::{Key, ModelStore};

use crate::intent::Intent;
use crate::route::Ctx;
use crate::validate::{self, ItemRef, entity_id};

/// The `yield_` of `Command::CityPurchase` for gold and for faith (the yield types of the core).
const YIELD_GOLD: i8 = 2;
const YIELD_FAITH: i8 = 5;

/// Run a `production.*` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(city), name) = (intent.key("city"), intent.name()) else {
        return;
    };
    // The player's own city, with the window of it in the model.
    let model: &ModelStore = ctx.model;
    if validate::own_city(model, city).is_none() {
        ctx.refuse(RefuseWhy::NotYourCity);
        return;
    }
    let window = validate::production_of(model, city);
    match name {
        "production.choose" | "production.replace" | "production.push" => {
            let Some(item) = intent.key("item") else {
                return;
            };
            let Some((item_ref, _)) = enabled_option(window, item) else {
                ctx.refuse(RefuseWhy::OptionUnavailable);
                return;
            };
            let Some(window) = window else {
                return;
            };
            let (cmd, target) = match name {
                "production.choose" => (
                    Command::CitySetProduction {
                        city: entity_id(city),
                        order: item_ref.order,
                        item: item_ref.item,
                    },
                    item,
                ),
                "production.replace" => {
                    let has_head =
                        window.current.is_some() || window.queue.iter().any(|r| r.slot == 0);
                    if !has_head {
                        ctx.refuse(RefuseWhy::BadSlot);
                        return;
                    }
                    (
                        Command::CityReplaceHead {
                            city: entity_id(city),
                            order: item_ref.order,
                            item: item_ref.item,
                        },
                        item,
                    )
                }
                _ => {
                    if !window.queue_known {
                        ctx.refuse(RefuseWhy::Unsupported);
                        return;
                    }
                    let front =
                        intent.variant("at").and_then(PushAt::from_u16) == Some(PushAt::Front);
                    (
                        Command::CityQueuePush {
                            city: entity_id(city),
                            order: item_ref.order,
                            item: item_ref.item,
                            front,
                        },
                        item,
                    )
                }
            };
            ctx.send(cmd, name, option_target(city, target), None);
        }
        "production.pop" | "production.move" => queue_edit(ctx, intent, city, window),
        "production.buy" => {
            let (Some(item), Some(with)) = (
                intent.key("item"),
                intent.variant("with").and_then(BuyWith::from_u16),
            ) else {
                return;
            };
            buy(ctx, city, item, with, window);
        }
        _ => {}
    }
}

/// The option with this key if the window has it, can be built and its key fits an order.
fn enabled_option(window: Option<&Production>, item: Key) -> Option<(ItemRef, &Opt)> {
    let opt = validate::option_of(window?, item)?;
    if !opt.enabled {
        return None;
    }
    Some((ItemRef::from_key(item)?, opt))
}

/// Remove an entry of the queue or move one.
fn queue_edit(ctx: &mut Ctx<'_>, intent: &Intent, city: Key, window: Option<&Production>) {
    let Some(window) = window else {
        ctx.refuse(RefuseWhy::BadSlot);
        return;
    };
    if !window.queue_known {
        ctx.refuse(RefuseWhy::Unsupported);
        return;
    }
    let slot = |n: Option<i32>| {
        let n = n?;
        let ok = window.queue.iter().any(|r| r.slot == n);
        ok.then(|| i8::try_from(n).ok()).flatten()
    };
    let cmd = if intent.name() == "production.pop" {
        slot(intent.int("slot")).map(|slot| Command::CityQueuePop {
            city: entity_id(city),
            slot,
        })
    } else {
        let (from, to) = (slot(intent.int("from")), slot(intent.int("to")));
        match (from, to) {
            (Some(from), Some(to)) if from != to => Some(Command::CityQueueMove {
                city: entity_id(city),
                from,
                to,
            }),
            _ => None,
        }
    };
    match cmd {
        Some(cmd) => {
            ctx.send(cmd, intent.name(), city_target(city), None);
        }
        None => ctx.refuse(RefuseWhy::BadSlot),
    }
}

/// Buy an item with gold or faith, if the model says the player can.
fn buy(ctx: &mut Ctx<'_>, city: Key, item: Key, with: BuyWith, window: Option<&Production>) {
    let Some(opt) = window.and_then(|w| validate::option_of(w, item)) else {
        ctx.refuse(RefuseWhy::OptionUnavailable);
        return;
    };
    let (price, ok, yield_) = match with {
        BuyWith::Gold => (opt.buy_gold, opt.buy_gold_ok, YIELD_GOLD),
        BuyWith::Faith => (opt.buy_faith, opt.buy_faith_ok, YIELD_FAITH),
    };
    let Some(item_ref) = ItemRef::from_key(item) else {
        ctx.refuse(RefuseWhy::OptionUnavailable);
        return;
    };
    if price.is_none() || !ok {
        ctx.refuse(RefuseWhy::CannotBuy);
        return;
    }
    ctx.send(
        Command::CityPurchase {
            city: entity_id(city),
            order: item_ref.order,
            item: item_ref.item,
            yield_,
        },
        "production.buy",
        option_target(city, item),
        None,
    );
}
