//! The contexts the server precomputes for the units and the city in focus (net-protocol 4.9): what the
//! bot keeps of `GameMsg::Context`. A context is valid while its `view_seq` equals the bot's `view_seq`.

use std::collections::BTreeMap;

use protocol::{CityContext, ContextMsg, UnitContext};

#[derive(Debug, Default, Clone)]
pub struct Contexts {
    pub units: BTreeMap<i32, UnitContext>,
    pub city: Option<CityContext>,
}

impl Contexts {
    pub fn apply(&mut self, m: &ContextMsg, _view_seq: u64) {
        match m {
            ContextMsg::Unit(u) => {
                self.units.insert(u.unit, u.clone());
            }
            ContextMsg::City(c) => self.city = Some(c.clone()),
            ContextMsg::Valid {
                units,
                city,
                view_seq,
            } => {
                for id in units {
                    if let Some(u) = self.units.get_mut(id) {
                        u.view_seq = *view_seq;
                    }
                }
                if let (Some(id), Some(c)) = (city, self.city.as_mut())
                    && c.city == *id
                {
                    c.view_seq = *view_seq;
                }
            }
            ContextMsg::Dropped { unit } => {
                self.units.remove(unit);
            }
        }
    }

    /// The context of `unit` when it is valid for the view at `view_seq`.
    pub fn unit(&self, unit: i32, view_seq: u64) -> Option<&UnitContext> {
        self.units.get(&unit).filter(|c| c.view_seq == view_seq)
    }

    /// The context of `city` when it is valid for the view at `view_seq`.
    pub fn city(&self, city: i32, view_seq: u64) -> Option<&CityContext> {
        self.city
            .as_ref()
            .filter(|c| c.city == city && c.view_seq == view_seq)
    }
}
