use redis::Client;

pub fn init_client(redis_url: &str) -> Client {
    Client::open(redis_url).expect("Failed to connect to Redis")
}
