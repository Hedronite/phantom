mod common;

use common::*;

#[test]
fn second_case() {
    common::fixture_data(); // (a) a later claimant
    fixture_data(); // (a) and (b): the glob of the later claimant's row
}
