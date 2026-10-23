//! This intentionally failing, ignored integration test is executed ONLY by
//! the expected-failure CI gate. Ordinary local and CI workspace tests stay green.
//!
//! This checks that Cargo's test runner actually reports an assertion failure
//! rather than allowing broken tests to be mistaken for successful execution.

#[test]
#[ignore = "Only the CI expected-failure gate should execute this test"]
fn deliberate_ci_failure_probe() {
    panic!("BLUEROUTE_EXPECTED_TEST_FAILURE");
}
