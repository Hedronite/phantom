pub mod app;

pub fn top_helper() {}

pub fn outer_alpha() {
    fn shared_helper() {
        top_helper();
    }
    shared_helper();
}

pub fn outer_beta() {
    fn shared_helper() {
        crate::app::aux();
    }
    shared_helper();
}

pub fn layered() {
    fn mid() {
        fn leaf() {
            top_helper();
        }
        leaf();
    }
    mid();
}
