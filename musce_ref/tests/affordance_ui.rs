#[test]
fn invalid_affordance_declarations_fail_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/affordance/*.rs");
}
