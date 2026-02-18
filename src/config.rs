use std::env;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub read_database_url: String,
    pub redis_url: String,
    pub http_port: u16,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        Self {
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL missing"),
            read_database_url: env::var("READ_DATABASE_URL").expect("READ_DATABASE_URL missing"),
            redis_url: env::var("REDIS_URL").expect("REDIS_URL missing"),
            http_port: env::var("HTTP_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .expect("Invalid HTTP_PORT"),
        }
    }
}
