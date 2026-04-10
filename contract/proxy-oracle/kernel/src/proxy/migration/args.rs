use near_sdk::near;

#[derive(Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "from_version", rename_all = "snake_case")]
pub enum MigrationArgs {
    V0,
}
