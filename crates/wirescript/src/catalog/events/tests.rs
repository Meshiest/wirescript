    use super::*;

    #[test]
    fn all_events_registered() {
        assert_eq!(events().len(), 25);
        assert!(find_event("Loaded").is_some());
        assert!(find_event("RoundStart").is_some());
        assert!(find_event("WholeGridInteracted").is_some());
        assert!(find_event("WholeGridTargeted").is_some());
        assert!(find_event("Clock").is_some());
        assert!(find_event("GlobalCustomEvent").is_some());
        assert!(find_event("ControllerJoinedTeam").is_some());
        assert!(find_event("ControllerLeftTeam").is_some());
        assert!(find_event("CustomEvent").is_some());
        assert!(find_event("CharacterSpawned").is_some());
        assert!(find_event("ChatCommand").is_some());
        assert!(find_event("CharacterDamaged").is_some());
        assert!(find_event("EntityZoneEntered").is_some());
        assert!(find_event("ProjectileZoneLeft").is_some());
        assert!(find_event("Nonexistent").is_none());
    }

    #[test]
    fn loaded_event_has_no_data_and_fires_from_exec_out() {
        let e = find_event("Loaded").unwrap();
        assert_eq!(e.gate_class, "BrickComponentType_Event_Loaded");
        assert_eq!(e.exec_out, "ExecOut");
        assert!(e.data.is_empty());
        assert!(e.input_named.is_empty());
    }

    #[test]
    fn brick_zone_events_bind_brick_and_character() {
        for name in ["BrickChanged", "BrickRemoved"] {
            let e = find_event(name).unwrap();
            let data: Vec<_> = e.data.iter().map(|d| (d.name, d.port, d.ty.clone())).collect();
            assert_eq!(
                data,
                [("brick", "Brick", Type::Brick), ("character", "Character", Type::Character)],
                "{name}"
            );
        }
    }

    #[test]
    fn character_spawned_has_character_binding() {
        let e = find_event("CharacterSpawned").unwrap();
        assert_eq!(e.data.len(), 1);
        assert_eq!(e.data[0].name, "character");
        assert_eq!(e.data[0].port, "Character");
        assert!(matches!(e.data[0].ty, Type::Character));
    }
