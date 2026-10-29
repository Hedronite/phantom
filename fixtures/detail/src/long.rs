//! Long signatures and a non-ASCII doc near the cut.

/// Takes far too many arguments.
pub fn configure_everything(
    first_argument_with_a_long_name: u64,
    second_argument_with_a_long_name: u64,
    third_argument_with_a_long_name: u64,
    fourth_argument_with_a_long_name: u64,
    fifth_argument_with_a_long_name: u64,
    sixth_argument_with_a_long_name: u64,
) -> u64 {
    first_argument_with_a_long_name + second_argument_with_a_long_name
}

/// Exactly at the signature cap, so nothing is cut.
pub fn fits_the_cap(
    alpha_value: u64,
    beta_value: u64,
    gamma_value: u64,
    delta_value: u64,
    epsilon_value: u64,
    zeta_value: u64,
    eta_value: u64,
    theta_value: u64,
    iota_value: u64,
    kappa_value: u64,
    lambda_value: u64,
    omicron_value: u64,
    mu_v: u64,
) -> u64 {
    alpha_value + beta_value
}

/// Résumé of a run, written for the reader who skims the page: every field is kept as measured, nothing is rounded, and the totals come last, as they did in a café → past the cut. Then a second sentence.
pub fn summarize() -> &'static str {
    "done"
}
