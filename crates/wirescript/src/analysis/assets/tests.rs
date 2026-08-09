    use super::*;

    #[test]
    fn asset_type_for_port_maps_known_asset_params() {
        assert_eq!(asset_type_for_port("Font"), Some("BrickFontDescriptor"));
        assert_eq!(asset_type_for_port("ItemTypeIfItem"), Some("BRItemBase"));
        assert_eq!(asset_type_for_port("ProjectileOverride"), Some("BRWeaponProjectile"));
        // Wire-input / non-asset ports have no mapping.
        assert_eq!(asset_type_for_port("IgnoreEntity"), None);
        assert_eq!(asset_type_for_port("Team"), None);
        // Every mapped type is actually present in the catalog.
        for port in ["Font", "ItemType", "ProjectileOverride", "AudioDescriptor"] {
            let ty = asset_type_for_port(port).unwrap();
            assert!(asset_type_exists(ty), "{ty} not in catalog");
            assert!(!asset_names(ty).is_empty(), "{ty} has no names");
        }
    }
