use eww_shared_util::VarName;
use meh_core::VarState;
use simplexpr::dynval::DynVal;

#[test]
fn set_skips_unchanged_values() {
    let mut state = VarState::new();
    let name = VarName("FOO".to_string());
    assert!(state.set(name.clone(), DynVal::from_string("bar".into())));
    assert!(!state.set(name.clone(), DynVal::from_string("bar".into())));
    assert!(state.set(name, DynVal::from_string("baz".into())));
}
