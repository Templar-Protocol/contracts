use sqlx::{postgres::PgPoolOptions, PgPool};

#[derive(Debug, Clone)]
pub struct Database {
    connection: PgPool,
}

impl Database {
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let connection = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(database_url)?;

        Ok(Self { connection })
    }
}
