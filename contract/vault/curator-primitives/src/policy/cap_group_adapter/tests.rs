    use super::*;

    const WAD: u128 = 1_000_000_000_000_000_000_000_000;

    #[test]
    fn builds_cap_group_and_record_from_fields() {
        let cap = cap_group_from_fields(1_000, Wad::from(WAD / 2));
        assert_eq!(cap.absolute_cap.map(|v| v.get()), Some(1_000));
        assert_eq!(cap.relative_cap, Some(Wad::from(WAD / 2)));

        let record = cap_group_record_from_fields(1_000, Wad::from(WAD / 2), 300);
        assert_eq!(record.principal, 300);
        assert_eq!(record.cap.absolute_cap.map(|v| v.get()), Some(1_000));
    }

    #[test]
    fn alloc_helpers_match_cap_group_behavior() {
        assert!(can_allocate_from_fields(1_000, Wad::one(), 300, 500, 2_000));
        assert!(!can_allocate_from_fields(
            1_000,
            Wad::one(),
            300,
            800,
            2_000
        ));

        assert!(enforce_from_fields(1_000, Wad::one(), 300, 500, 2_000).is_ok());
        assert!(enforce_from_fields(1_000, Wad::one(), 300, 800, 2_000).is_err());
    }

    #[test]
    fn computes_effective_and_available_from_fields() {
        assert_eq!(effective_cap_from_fields(1_000, Wad::one(), 500), 500);
        assert_eq!(
            available_capacity_from_fields(1_000, Wad::one(), 300, 500),
            200
        );
    }
