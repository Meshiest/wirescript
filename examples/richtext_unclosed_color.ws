// Probes whether an unclosed <color> tag keeps applying until the next one.
//
// This is the assumption a colour run-length encoder rests on: emit a tag only
// when the colour changes, and let it span every character until the next tag.
// If the parser instead requires </> and literalises or drops unterminated
// markup, that scheme renders as visible tag text rather than colour.
//
// Every existing use of <color> in this repo closes with </>, so there is no
// prior art either way. Paste into a grid, walk up to the text, and read.
//
// Line 1 is the run-length shape under test. Line 2 is the same content with
// explicit closes, as a control: if 1 and 2 look identical the scheme works.
// Line 3 asks whether colour survives <br>, which decides whether rows can be
// joined into one string. Line 4 is depth, one row's worth of opens.

var player: character

on CharacterSpawned() -> (character) {
  player = character
}

in probe: exec

on probe {
  // Two opens, no close. Red then blue is the encoder's actual output shape.
  player.DisplayText(
    '1 unclosed: <color="FF0000">RED<color="0000FF">BLUE',
    positionY = -60.0, fontSize = 24, lifetime = 0.0, textId = 1)

  // The same content closed properly. The control.
  player.DisplayText(
    '2 closed:   <color="FF0000">RED</><color="0000FF">BLUE</>',
    positionY = -30.0, fontSize = 24, lifetime = 0.0, textId = 2)

  // Colour crossing a line break decides whether rows join into one string.
  player.DisplayText(
    '3 over br:  <color="00FF00">GREEN<br>STILL GREEN?',
    positionY = 0.0, fontSize = 24, lifetime = 0.0, textId = 3)

  // Sixty opens is one display row. If depth is capped this is where it shows.
  var deep: string = "4 deep:     "
  for i in 0..60 {
    let hex = if (i & 1) == 0 then "FF0000" else "0000FF"
    deep = deep .. '<color="${hex}">' .. "█"
  }
  player.DisplayText(deep, positionY = 30.0, fontSize = 24, lifetime = 0.0, textId = 4)

  BroadcastChatMessage('chat unclosed: <color="FF0000">RED<color="0000FF">BLUE')
  BroadcastChatMessage('chat closed:   <color="FF0000">RED</><color="0000FF">BLUE</>')
}
