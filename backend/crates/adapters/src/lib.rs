use itinera_core::ports::ids::IdGen;

pub mod memory;

pub struct UuidIdGen {}

impl IdGen for UuidIdGen {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
