//! Health checks for the platform: `live` means the process answers, `ready` means
//! it can serve traffic. They are the same until there are dependencies to wait for.

pub async fn live() -> &'static str {
    "ok"
}

pub async fn ready() -> &'static str {
    "ok"
}
