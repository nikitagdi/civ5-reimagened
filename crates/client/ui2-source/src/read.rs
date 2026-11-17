//! Read-only inputs of the HUD builders.

pub use crate::records::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("the source does not serve this field")]
    Unsupported,
    #[error("world read failed: {0}")]
    Core(String),
}

/// Read-only projection records and revision-scoped protocol answers.
pub trait GameRead {
    fn seat(&self) -> u8;
    fn team(&self) -> i32;
    fn game_info(&mut self) -> Result<GameInfo, ReadError>;
    fn players(&mut self) -> Result<Vec<Player>, ReadError>;
    fn catalog(&mut self, table: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError>;
    fn units(&mut self) -> Result<Vec<Unit>, ReadError>;
    fn unit_actions(&mut self, unit: i32) -> Result<Vec<UnitAction>, ReadError>;
    fn cities(&mut self) -> Result<Vec<City>, ReadError>;
    fn city_builds(&mut self, city: i32) -> Result<Vec<CityBuild>, ReadError>;
    fn top_bar(&mut self) -> Result<TopBar, ReadError>;
    fn notifications(&mut self) -> Result<Vec<Notification>, ReadError>;
    fn city_queue(&mut self, city: i32) -> Result<Vec<CityOrder>, ReadError>;
    fn techs(&mut self) -> Result<Vec<TechState>, ReadError>;
    fn player_resources(&mut self) -> Result<Vec<PlayerResource>, ReadError>;
    fn policy_branches(&mut self) -> Result<Vec<PolicyBranchState>, ReadError>;
    fn unit_promotions(&mut self, unit: i32) -> Result<Vec<i16>, ReadError>;
}
