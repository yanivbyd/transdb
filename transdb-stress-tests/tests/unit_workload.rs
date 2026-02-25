use transdb_stress_tests::workload::{Op, WorkloadProfile};

#[test]
fn test_profile_boundaries() {
    // ReadHeavy: GET rolls 0–79, PUT rolls 80–99
    assert_eq!(WorkloadProfile::ReadHeavy.op_for_roll(0), Op::Get);
    assert_eq!(WorkloadProfile::ReadHeavy.op_for_roll(79), Op::Get);
    assert_eq!(WorkloadProfile::ReadHeavy.op_for_roll(80), Op::Put);
    assert_eq!(WorkloadProfile::ReadHeavy.op_for_roll(99), Op::Put);

    // Balanced: GET 0–49, PUT 50–94, DELETE 95–99
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(0), Op::Get);
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(49), Op::Get);
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(50), Op::Put);
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(94), Op::Put);
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(95), Op::Delete);
    assert_eq!(WorkloadProfile::Balanced.op_for_roll(99), Op::Delete);

    // WriteHeavy: GET 0–19, PUT 20–94, DELETE 95–99
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(0), Op::Get);
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(19), Op::Get);
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(20), Op::Put);
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(94), Op::Put);
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(95), Op::Delete);
    assert_eq!(WorkloadProfile::WriteHeavy.op_for_roll(99), Op::Delete);

    // PutOnly: every roll is a PUT
    assert_eq!(WorkloadProfile::PutOnly.op_for_roll(0), Op::Put);
    assert_eq!(WorkloadProfile::PutOnly.op_for_roll(99), Op::Put);
}
