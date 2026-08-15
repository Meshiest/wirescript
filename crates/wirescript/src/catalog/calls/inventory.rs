//! Inventory and ammo calls.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Character inventory -------------------------------
    // `char.GiveWeapon($BRItemBase/Weapon_Pistol, slot)` — sets an inventory
    // slot to an item asset. The weapon asset is carried as the nested
    // EntryPlan.ItemTypeIfItem; the emitter builds the EntryPlan struct.
    m.insert(
        "GiveWeapon",
        character_exec(
            "GiveWeapon",
            gc::CHARACTER_SET_INVENTORY_ENTRY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("weapon", WirePort::ItemTypeIfItem, Type::Entity),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );

    // ---- Character inventory family ------------------------------
    // Asset args ($BRItemBase/..., $BrickTypeAsset/..., entity types) inline
    // into the gate's class/object data fields (like GiveWeapon).
    m.insert(
        "AddInventoryItem",
        character_exec(
            "AddInventoryItem",
            gc::CHARACTER_ADD_INVENTORY_ITEM,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::Item, Type::Entity),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryItem",
        character_exec(
            "SetInventoryItem",
            gc::CHARACTER_SET_INVENTORY_ITEM,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::Item, Type::Entity),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryBrick",
        character_exec(
            "AddInventoryBrick",
            gc::CHARACTER_ADD_INVENTORY_BRICK,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("brick", WirePort::BrickAsset, Type::Entity),
                CallParam::opt("size", WirePort::ProceduralSize, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryBrick",
        character_exec(
            "SetInventoryBrick",
            gc::CHARACTER_SET_INVENTORY_BRICK,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("brick", WirePort::BrickAsset, Type::Entity),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
                CallParam::opt("size", WirePort::ProceduralSize, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryEntity",
        character_exec(
            "AddInventoryEntity",
            gc::CHARACTER_ADD_INVENTORY_ENTITY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("entityType", WirePort::EntityType, Type::Entity),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryEntity",
        character_exec(
            "SetInventoryEntity",
            gc::CHARACTER_SET_INVENTORY_ENTITY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("entityType", WirePort::EntityType, Type::Entity),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryItemAdv",
        character_exec(
            "AddInventoryItemAdv",
            gc::CHARACTER_ADD_INVENTORY_ITEM_ADV,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::ItemType, Type::Entity),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
                CallParam::opt("speed", WirePort::WeaponSpeedMultiplier, Type::Float),
                CallParam::opt("scale", WirePort::ItemScale, Type::Float),
                CallParam::opt("itemName", WirePort::ItemNameOverride, Type::String),
                CallParam::opt("projectile", WirePort::ProjectileOverride, Type::Entity),
                // Config-only (settings menu, not wire inputs). `meshColors` is
                // a color array (`Color[]`); `ammoOverride` is the
                // WeaponAmmoOverride nested struct — both constant-only.
                CallParam::opt("overrideColors", WirePort::BOverrideColors, Type::Bool),
                CallParam::opt("meshColors", WirePort::MeshColors, mesh_colors_type()),
                CallParam::opt("ammoOverride", WirePort::WeaponAmmoOverride, ammo_override_type()),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryItemAdv",
        character_exec(
            "SetInventoryItemAdv",
            gc::CHARACTER_SET_INVENTORY_ITEM_ADV,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::ItemType, Type::Entity),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
                CallParam::opt("speed", WirePort::WeaponSpeedMultiplier, Type::Float),
                CallParam::opt("scale", WirePort::ItemScale, Type::Float),
                CallParam::opt("itemName", WirePort::ItemNameOverride, Type::String),
                CallParam::opt("projectile", WirePort::ProjectileOverride, Type::Entity),
                // Config-only (settings menu, not wire inputs). `meshColors` is
                // a color array (`Color[]`); `ammoOverride` is the
                // WeaponAmmoOverride nested struct — both constant-only.
                CallParam::opt("overrideColors", WirePort::BOverrideColors, Type::Bool),
                CallParam::opt("meshColors", WirePort::MeshColors, mesh_colors_type()),
                CallParam::opt("ammoOverride", WirePort::WeaponAmmoOverride, ammo_override_type()),
            ],
            vec![],
        ),
    );

    // ---- Character ammo / inventory --------------------------------------
    m.insert(
        "GetAmmo",
        character_exec(
            "GetAmmo",
            gc::CHARACTER_GET_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
            ],
            vec![CallOutput {
                field: None,
                port: WirePort::Amount,
                ty: Type::Int,
            }],
        ),
    );
    m.insert(
        "GrantAmmo",
        character_exec(
            "GrantAmmo",
            gc::CHARACTER_GRANT_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
                CallParam::req("amount", WirePort::Amount, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetAmmo",
        character_exec(
            "SetAmmo",
            gc::CHARACTER_SET_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
                CallParam::req("amount", WirePort::Amount, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetInventoryEntry",
        character_exec(
            "GetInventoryEntry",
            gc::CHARACTER_GET_INVENTORY_ENTRY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("slot", WirePort::Slot, Type::Int),
            ],
            vec![CallOutput {
                field: None,
                port: WirePort::Item,
                ty: Type::Record(vec![
                    ("Item".into(), Type::Entity),
                    ("BrickAsset".into(), Type::Entity),
                    ("EntityType".into(), Type::Entity),
                ]),
            }],
        ),
    );
    m.insert(
        "GetCurrentInventorySlot",
        character_exec(
            "GetCurrentInventorySlot",
            gc::CHARACTER_GET_CURRENT_INVENTORY_SLOT,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
                field: None,
                port: WirePort::Slot,
                ty: Type::Int,
            }],
        ),
    );
    m.insert(
        "GetWeaponChamberAmmo",
        character_exec(
            "GetWeaponChamberAmmo",
            gc::CHARACTER_GET_WEAPON_CHAMBER_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
                CallParam::req("slot", WirePort::Slot, Type::Int),
            ],
            vec![CallOutput {
                field: None,
                port: WirePort::Amount,
                ty: Type::Int,
            }],
        ),
    );
    m.insert(
        "IncWeaponChamberAmmo",
        character_exec(
            "IncWeaponChamberAmmo",
            gc::CHARACTER_INC_WEAPON_CHAMBER_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
                CallParam::req("slot", WirePort::Slot, Type::Int),
                CallParam::req("amount", WirePort::Amount, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetWeaponChamberAmmo",
        character_exec(
            "SetWeaponChamberAmmo",
            gc::CHARACTER_SET_WEAPON_CHAMBER_AMMO,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("resource", WirePort::Resource, Type::Entity),
                CallParam::req("slot", WirePort::Slot, Type::Int),
                CallParam::req("amount", WirePort::Amount, Type::Int),
            ],
            vec![],
        ),
    );

    m.insert(
        "ItemToPickup",
        expr_recv(
            "ItemToPickup",
            gc::EXPR_ITEM_TO_PICKUP,
            Type::Entity,
            vec![CallParam::req("item", WirePort::Input, Type::Entity)],
            WirePort::Output,
            Type::Entity,
        ),
    );
}

/// `meshColors` config type: an array of colors (`Color[]`) for the
/// `AddInventoryItemAdv` / `SetInventoryItemAdv` `MeshColors` data field.
fn mesh_colors_type() -> Type {
    Type::Array(Box::new(Type::Color))
}

/// `ammoOverride` config type: the `WeaponAmmoOverride` nested struct as a record
/// `{ overrideStartingAmmo: bool, resources: [{ loaded: int, reserve: int }] }`.
/// Constant-only; validated and folded at the call site.
fn ammo_override_type() -> Type {
    Type::Record(vec![
        ("overrideStartingAmmo".into(), Type::Bool),
        (
            "resources".into(),
            Type::Array(Box::new(Type::Record(vec![
                ("loaded".into(), Type::Int),
                ("reserve".into(), Type::Int),
            ]))),
        ),
    ])
}
