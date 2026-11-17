//! Typed projection of the schema-validated service root into HUD reads.
use crate::read::*;
use crate::world::HudExtras;
use ui2_model::Text;
use ui2_model::generated::{Hud, HudCity, HudUnit};

#[derive(Default)]
pub struct HudScreen {
    root: Option<Hud>,
    revision: u64,
}

impl HudScreen {
    pub fn update(&mut self, root: Option<&Hud>) {
        if self.root.as_ref() != root {
            self.root = root.cloned();
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn root(&self) -> Result<&Hud, ReadError> {
        self.root.as_ref().ok_or(ReadError::Unsupported)
    }

    fn city(&self, id: i32) -> Result<&HudCity, ReadError> {
        self.root()?
            .city
            .as_ref()
            .filter(|c| c.id == id)
            .ok_or(ReadError::Unsupported)
    }

    fn unit(&self, id: i32) -> Result<&HudUnit, ReadError> {
        self.root()?
            .unit
            .as_ref()
            .filter(|u| u.id == id)
            .ok_or(ReadError::Unsupported)
    }
}

fn text(value: &Text) -> String {
    match value {
        Text::Raw(s) => s.to_string(),
        Text::Key { key, .. } => key.to_string(),
    }
}

impl HudExtras for HudScreen {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn policy_branch_catalog(&self) -> Vec<CatalogEntry> {
        let Ok(root) = self.root() else {
            return Vec::new();
        };
        root.top
            .policies
            .iter()
            .map(|p| CatalogEntry {
                id: p.id,
                type_name: p.type_name.to_string(),
                name: text(&p.name),
                icon_index: None,
                flag_index: None,
                art_define: None,
                icon_atlas: None,
                flag_atlas: None,
                art_style: None,
                strategic_view: None,
            })
            .collect()
    }

    fn city_queue(&self, city: i32) -> Result<Vec<CityOrder>, ReadError> {
        Ok(self
            .city(city)?
            .queue
            .iter()
            .map(|q| CityOrder {
                slot: q.slot,
                order: OrderType::from_abi(q.order),
                item: q.item,
                turns: q.turns,
                needed: q.needed,
            })
            .collect())
    }

    fn city_build_reason(&self, city: i32, order: OrderType, item: i16) -> Option<String> {
        self.city(city)
            .ok()?
            .reasons
            .iter()
            .find(|r| i32::from(r.order) == order.abi() && r.item == item)
            .map(|r| text(&r.reason))
    }

    fn city_overflow(&self, city: i32) -> Option<i32> {
        self.city(city).ok().map(|c| c.overflow)
    }

    fn techs(&self) -> Result<Vec<TechState>, ReadError> {
        Ok(self
            .root()?
            .research
            .techs
            .iter()
            .map(|t| TechState {
                tech: t.id,
                status: TechStatus::from_abi(t.status.cast_unsigned()),
                queue: t.queue.and_then(|q| u8::try_from(q).ok()),
                turns: t.turns,
                progress: t.progress,
                cost: t.cost,
            })
            .collect())
    }

    fn player_resources(&self) -> Result<Vec<PlayerResource>, ReadError> {
        Ok(self
            .root()?
            .top
            .resources
            .iter()
            .map(|r| PlayerResource {
                resource: r.id,
                usage: ResourceUsage::from_abi(r.usage.cast_unsigned()),
                available: r.available,
                used: r.used,
                total: r.total,
            })
            .collect())
    }

    fn policy_branches(&self) -> Result<Vec<PolicyBranchState>, ReadError> {
        Ok(self
            .root()?
            .top
            .policies
            .iter()
            .map(|p| PolicyBranchState {
                branch: p.id,
                unlocked: p.unlocked,
                owned: p.owned.cast_unsigned(),
                total: p.total.cast_unsigned(),
            })
            .collect())
    }

    fn unit_promotions(&self, unit: i32) -> Result<Vec<i16>, ReadError> {
        Ok(self.unit(unit)?.promotions.iter().map(|p| p.id).collect())
    }

    fn unit_actions(&self, unit: i32) -> Vec<UnitAction> {
        let Ok(unit) = self.unit(unit) else {
            return Vec::new();
        };
        unit.actions
            .iter()
            .map(|a| UnitAction {
                action: a.id,
                enabled: a.enabled,
                reason: a.reason.as_ref().map(text),
                cost: a.cost,
            })
            .collect()
    }

    fn tourism(&self) -> Option<i32> {
        self.root().ok()?.top.tourism
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_model::{ModelStore, Sample, Schema};

    #[test]
    fn service_fixture_supplies_all_extra_reads_and_tracks_replacement() {
        let sample = Sample::parse(
            Schema::v1(),
            include_str!("../../../../skins/engraved/sample/hud.json"),
        )
        .unwrap();
        let mut store = ModelStore::new(Schema::v1());
        sample.apply(&mut store).unwrap();
        let root = store.get::<Option<Hud>>().unwrap().as_ref().unwrap();
        let mut extras = HudScreen::default();
        assert_eq!(extras.techs(), Err(ReadError::Unsupported));
        extras.update(Some(root));
        let revision = extras.revision();
        extras.update(Some(root));
        assert_eq!(extras.revision(), revision);
        let queue = extras.city_queue(3).unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(
            (
                queue[1].slot,
                queue[1].order,
                queue[1].item,
                queue[1].turns,
                queue[1].needed
            ),
            (1, OrderType::Construct, 4, Some(7), 100)
        );
        assert_eq!(extras.city_overflow(3), Some(7));
        assert!(
            extras
                .city_build_reason(3, OrderType::Construct, 4)
                .is_some()
        );
        assert_eq!(extras.city_build_reason(3, OrderType::Train, 4), None);
        let techs = extras.techs().unwrap();
        assert_eq!(techs[1].queue, Some(1));
        assert_eq!(techs[1].status, TechStatus::CanResearch);
        assert_eq!(
            extras.player_resources().unwrap()[1].usage,
            ResourceUsage::Luxury
        );
        assert_eq!(extras.policy_branches().unwrap()[0].owned, 2);
        assert_eq!(extras.policy_branch_catalog()[0].name, "Tradition");
        assert_eq!(extras.tourism(), Some(4));
        assert_eq!(extras.unit_promotions(4).unwrap(), [2]);
        assert_eq!(extras.unit_actions(4)[2].cost, Some(210));
        assert!(!extras.unit_actions(4)[1].enabled);
        assert_eq!(extras.city_queue(99), Err(ReadError::Unsupported));
        assert_eq!(extras.unit_promotions(99), Err(ReadError::Unsupported));
        assert!(extras.unit_actions(99).is_empty());
        let mut replacement = root.clone();
        replacement.city = None;
        replacement.unit = None;
        extras.update(Some(&replacement));
        assert!(extras.revision() > revision);
        assert_eq!(extras.city_queue(3), Err(ReadError::Unsupported));
        assert_eq!(extras.unit_promotions(4), Err(ReadError::Unsupported));
        extras.update(None);
        assert_eq!(extras.tourism(), None);
        assert_eq!(extras.techs(), Err(ReadError::Unsupported));
    }
}
