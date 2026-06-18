use crate::NearClient;

pub trait HasNearClient: Clone + Send + Sync + 'static {
    fn near_client(&self) -> &NearClient;
}
