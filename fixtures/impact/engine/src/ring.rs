//! A cycle, and a function that calls itself.

pub fn ping(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    pong(n - 1)
}

pub fn pong(n: u32) -> u32 {
    ping(n)
}

pub fn again(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    again(n - 1) + ping(n)
}
