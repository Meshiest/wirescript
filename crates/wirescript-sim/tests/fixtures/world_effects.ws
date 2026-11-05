// Audio, explosions and SweepSimple, driven by world_effects.script. Sounds
// and explosions print as lines; the asset is its index in the save.
on ChatCommand("boom", "Makes noise") -> (controller, arguments) {
  let c = controller.CharacterOf()
  PlayGlobalAudio($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press)
  c.PlayClientAudio($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press, volume = 0.5)
  c.PlayAudioAt($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_2_Press, pitch = 2.0)
  SpawnExplosion($BRWeaponProjectile/Projectile_Bazooka, instigator = c, offset = Vec(0.0, 0.0, 50.0))
  SpawnExplosionAt(c.GetLocation(), $BRWeaponProjectile/Projectile_Bazooka)
}

on ChatCommand("look", "Sweeps ahead") -> (controller, arguments) {
  let r = SweepSimple(1000.0)
  if r.Hit {
    BroadcastChatMessage("hit at ${r.HitDistance}")
  } else {
    BroadcastChatMessage("missed")
  }
}
