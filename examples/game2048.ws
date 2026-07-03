// 2048 Game — wirescript implementation

in running: bool
in player: character
var ctrl: controller
var char: character

var c0: int = 0
var c1: int = 0
var c2: int = 0
var c3: int = 0
var c4: int = 0
var c5: int = 0
var c6: int = 0
var c7: int = 0
var c8: int = 0
var c9: int = 0
var c10: int = 0
var c11: int = 0
var c12: int = 0
var c13: int = 0
var c14: int = 0
var c15: int = 0

// tidN = text-element id for the tile currently at cell N. Moves with tiles
// across slides so DisplayText transitions animate tile motion smoothly.
var tid0: int = 1
var tid1: int = 2
var tid2: int = 3
var tid3: int = 4
var tid4: int = 5
var tid5: int = 6
var tid6: int = 7
var tid7: int = 8
var tid8: int = 9
var tid9: int = 10
var tid10: int = 11
var tid11: int = 12
var tid12: int = 13
var tid13: int = 14
var tid14: int = 15
var tid15: int = 16

let input = InputReader(char)
let goUp = running && input.Forward > 0.5
let goDown = running && input.Forward < -0.5
let goRight = running && input.Right > 0.5
let goLeft = running && input.Right < -0.5

on running {
  ctrl = ControllerOf(player)
  char = player
  c0 = 0; c1 = 0; c2 = 0; c3 = 0
  c4 = 0; c5 = 0; c6 = 0; c7 = 0
  c8 = 0; c9 = 0; c10 = 0; c11 = 0
  c12 = 0; c13 = 0; c14 = 0; c15 = 0
  tid0 = 1; tid1 = 2; tid2 = 3; tid3 = 4
  tid4 = 5; tid5 = 6; tid6 = 7; tid7 = 8
  tid8 = 9; tid9 = 10; tid10 = 11; tid11 = 12
  tid12 = 13; tid13 = 14; tid14 = 15; tid15 = 16
  let r1 = Random(0, 15)
  if r1 == 0 { c0 = 2 }
  if r1 == 1 { c1 = 2 }
  if r1 == 2 { c2 = 2 }
  if r1 == 3 { c3 = 2 }
  if r1 == 4 { c4 = 2 }
  if r1 == 5 { c5 = 2 }
  if r1 == 6 { c6 = 2 }
  if r1 == 7 { c7 = 2 }
  if r1 == 8 { c8 = 2 }
  if r1 == 9 { c9 = 2 }
  if r1 == 10 { c10 = 2 }
  if r1 == 11 { c11 = 2 }
  if r1 == 12 { c12 = 2 }
  if r1 == 13 { c13 = 2 }
  if r1 == 14 { c14 = 2 }
  if r1 == 15 { c15 = 2 }
}
on !running {
  DisplayText(ctrl, "", positionX = -180.0, positionY = -180.0, lifetime = 10.0, fontSize = 30, textId = 1)
  DisplayText(ctrl, "", positionX = -60.0, positionY = -180.0, lifetime = 10.0, fontSize = 30, textId = 2)
  DisplayText(ctrl, "", positionX = 60.0, positionY = -180.0, lifetime = 10.0, fontSize = 30, textId = 3)
  DisplayText(ctrl, "", positionX = 180.0, positionY = -180.0, lifetime = 10.0, fontSize = 30, textId = 4)
  DisplayText(ctrl, "", positionX = -180.0, positionY = -60.0, lifetime = 10.0, fontSize = 30, textId = 5)
  DisplayText(ctrl, "", positionX = -60.0, positionY = -60.0, lifetime = 10.0, fontSize = 30, textId = 6)
  DisplayText(ctrl, "", positionX = 60.0, positionY = -60.0, lifetime = 10.0, fontSize = 30, textId = 7)
  DisplayText(ctrl, "", positionX = 180.0, positionY = -60.0, lifetime = 10.0, fontSize = 30, textId = 8)
  DisplayText(ctrl, "", positionX = -180.0, positionY = 60.0, lifetime = 10.0, fontSize = 30, textId = 9)
  DisplayText(ctrl, "", positionX = -60.0, positionY = 60.0, lifetime = 10.0, fontSize = 30, textId = 10)
  DisplayText(ctrl, "", positionX = 60.0, positionY = 60.0, lifetime = 10.0, fontSize = 30, textId = 11)
  DisplayText(ctrl, "", positionX = 180.0, positionY = 60.0, lifetime = 10.0, fontSize = 30, textId = 12)
  DisplayText(ctrl, "", positionX = -180.0, positionY = 180.0, lifetime = 10.0, fontSize = 30, textId = 13)
  DisplayText(ctrl, "", positionX = -60.0, positionY = 180.0, lifetime = 10.0, fontSize = 30, textId = 14)
  DisplayText(ctrl, "", positionX = 60.0, positionY = 180.0, lifetime = 10.0, fontSize = 30, textId = 15)
  DisplayText(ctrl, "", positionX = 180.0, positionY = 180.0, lifetime = 10.0, fontSize = 30, textId = 16)
  DisplayText(ctrl, "", positionX = 0.0, positionY = 0.0, lifetime = 10.0, fontSize = 500, textId = 17)
  DisplayText(ctrl, "", positionX = -180.0, positionY = -280.0, lifetime = 10.0, fontSize = 60, textId = 18)
  DisplayText(ctrl, "", positionX = 180.0, positionY = -280.0, lifetime = 10.0, fontSize = 60, textId = 19)
}
on goLeft {
  if c0 == 0 { c0 = c1; c1 = c2; c2 = c3; c3 = 0; let temp = tid0; tid0 = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c0 == 0 { c0 = c1; c1 = c2; c2 = c3; c3 = 0; let temp = tid0; tid0 = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c0 == 0 { c0 = c1; c1 = c2; c2 = c3; c3 = 0; let temp = tid0; tid0 = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c1 == 0 { c1 = c2; c2 = c3; c3 = 0; let temp = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c1 == 0 { c1 = c2; c2 = c3; c3 = 0; let temp = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c2 == 0 { c2 = c3; c3 = 0; let temp = tid2; tid2 = tid3; tid3 = temp }
  if c0 == c1 { let temp = tid0; tid0 = tid1; tid1 = temp; c0 = c0 + c1; c1 = 0 }
  if c1 == c2 { let temp = tid1; tid1 = tid2; tid2 = temp; c1 = c1 + c2; c2 = 0 }
  if c2 == c3 { let temp = tid2; tid2 = tid3; tid3 = temp; c2 = c2 + c3; c3 = 0 }
  if c0 == 0 { c0 = c1; c1 = c2; c2 = c3; c3 = 0; let temp = tid0; tid0 = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c1 == 0 { c1 = c2; c2 = c3; c3 = 0; let temp = tid1; tid1 = tid2; tid2 = tid3; tid3 = temp }
  if c2 == 0 { c2 = c3; c3 = 0; let temp = tid2; tid2 = tid3; tid3 = temp }
  if c4 == 0 { c4 = c5; c5 = c6; c6 = c7; c7 = 0; let temp = tid4; tid4 = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c4 == 0 { c4 = c5; c5 = c6; c6 = c7; c7 = 0; let temp = tid4; tid4 = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c4 == 0 { c4 = c5; c5 = c6; c6 = c7; c7 = 0; let temp = tid4; tid4 = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c5 == 0 { c5 = c6; c6 = c7; c7 = 0; let temp = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c5 == 0 { c5 = c6; c6 = c7; c7 = 0; let temp = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c6 == 0 { c6 = c7; c7 = 0; let temp = tid6; tid6 = tid7; tid7 = temp }
  if c4 == c5 { let temp = tid4; tid4 = tid5; tid5 = temp; c4 = c4 + c5; c5 = 0 }
  if c5 == c6 { let temp = tid5; tid5 = tid6; tid6 = temp; c5 = c5 + c6; c6 = 0 }
  if c6 == c7 { let temp = tid6; tid6 = tid7; tid7 = temp; c6 = c6 + c7; c7 = 0 }
  if c4 == 0 { c4 = c5; c5 = c6; c6 = c7; c7 = 0; let temp = tid4; tid4 = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c5 == 0 { c5 = c6; c6 = c7; c7 = 0; let temp = tid5; tid5 = tid6; tid6 = tid7; tid7 = temp }
  if c6 == 0 { c6 = c7; c7 = 0; let temp = tid6; tid6 = tid7; tid7 = temp }
  if c8 == 0 { c8 = c9; c9 = c10; c10 = c11; c11 = 0; let temp = tid8; tid8 = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c8 == 0 { c8 = c9; c9 = c10; c10 = c11; c11 = 0; let temp = tid8; tid8 = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c8 == 0 { c8 = c9; c9 = c10; c10 = c11; c11 = 0; let temp = tid8; tid8 = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c9 == 0 { c9 = c10; c10 = c11; c11 = 0; let temp = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c9 == 0 { c9 = c10; c10 = c11; c11 = 0; let temp = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c10 == 0 { c10 = c11; c11 = 0; let temp = tid10; tid10 = tid11; tid11 = temp }
  if c8 == c9 { let temp = tid8; tid8 = tid9; tid9 = temp; c8 = c8 + c9; c9 = 0 }
  if c9 == c10 { let temp = tid9; tid9 = tid10; tid10 = temp; c9 = c9 + c10; c10 = 0 }
  if c10 == c11 { let temp = tid10; tid10 = tid11; tid11 = temp; c10 = c10 + c11; c11 = 0 }
  if c8 == 0 { c8 = c9; c9 = c10; c10 = c11; c11 = 0; let temp = tid8; tid8 = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c9 == 0 { c9 = c10; c10 = c11; c11 = 0; let temp = tid9; tid9 = tid10; tid10 = tid11; tid11 = temp }
  if c10 == 0 { c10 = c11; c11 = 0; let temp = tid10; tid10 = tid11; tid11 = temp }
  if c12 == 0 { c12 = c13; c13 = c14; c14 = c15; c15 = 0; let temp = tid12; tid12 = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c12 == 0 { c12 = c13; c13 = c14; c14 = c15; c15 = 0; let temp = tid12; tid12 = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c12 == 0 { c12 = c13; c13 = c14; c14 = c15; c15 = 0; let temp = tid12; tid12 = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c13 == 0 { c13 = c14; c14 = c15; c15 = 0; let temp = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c13 == 0 { c13 = c14; c14 = c15; c15 = 0; let temp = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c14 == 0 { c14 = c15; c15 = 0; let temp = tid14; tid14 = tid15; tid15 = temp }
  if c12 == c13 { let temp = tid12; tid12 = tid13; tid13 = temp; c12 = c12 + c13; c13 = 0 }
  if c13 == c14 { let temp = tid13; tid13 = tid14; tid14 = temp; c13 = c13 + c14; c14 = 0 }
  if c14 == c15 { let temp = tid14; tid14 = tid15; tid15 = temp; c14 = c14 + c15; c15 = 0 }
  if c12 == 0 { c12 = c13; c13 = c14; c14 = c15; c15 = 0; let temp = tid12; tid12 = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c13 == 0 { c13 = c14; c14 = c15; c15 = 0; let temp = tid13; tid13 = tid14; tid14 = tid15; tid15 = temp }
  if c14 == 0 { c14 = c15; c15 = 0; let temp = tid14; tid14 = tid15; tid15 = temp }
}
on goRight {
  if c3 == 0 { c3 = c2; c2 = c1; c1 = c0; c0 = 0; let temp = tid3; tid3 = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c3 == 0 { c3 = c2; c2 = c1; c1 = c0; c0 = 0; let temp = tid3; tid3 = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c3 == 0 { c3 = c2; c2 = c1; c1 = c0; c0 = 0; let temp = tid3; tid3 = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c2 == 0 { c2 = c1; c1 = c0; c0 = 0; let temp = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c2 == 0 { c2 = c1; c1 = c0; c0 = 0; let temp = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c1 == 0 { c1 = c0; c0 = 0; let temp = tid1; tid1 = tid0; tid0 = temp }
  if c3 == c2 { let temp = tid3; tid3 = tid2; tid2 = temp; c3 = c3 + c2; c2 = 0 }
  if c2 == c1 { let temp = tid2; tid2 = tid1; tid1 = temp; c2 = c2 + c1; c1 = 0 }
  if c1 == c0 { let temp = tid1; tid1 = tid0; tid0 = temp; c1 = c1 + c0; c0 = 0 }
  if c3 == 0 { c3 = c2; c2 = c1; c1 = c0; c0 = 0; let temp = tid3; tid3 = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c2 == 0 { c2 = c1; c1 = c0; c0 = 0; let temp = tid2; tid2 = tid1; tid1 = tid0; tid0 = temp }
  if c1 == 0 { c1 = c0; c0 = 0; let temp = tid1; tid1 = tid0; tid0 = temp }
  if c7 == 0 { c7 = c6; c6 = c5; c5 = c4; c4 = 0; let temp = tid7; tid7 = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c7 == 0 { c7 = c6; c6 = c5; c5 = c4; c4 = 0; let temp = tid7; tid7 = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c7 == 0 { c7 = c6; c6 = c5; c5 = c4; c4 = 0; let temp = tid7; tid7 = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c6 == 0 { c6 = c5; c5 = c4; c4 = 0; let temp = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c6 == 0 { c6 = c5; c5 = c4; c4 = 0; let temp = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c5 == 0 { c5 = c4; c4 = 0; let temp = tid5; tid5 = tid4; tid4 = temp }
  if c7 == c6 { let temp = tid7; tid7 = tid6; tid6 = temp; c7 = c7 + c6; c6 = 0 }
  if c6 == c5 { let temp = tid6; tid6 = tid5; tid5 = temp; c6 = c6 + c5; c5 = 0 }
  if c5 == c4 { let temp = tid5; tid5 = tid4; tid4 = temp; c5 = c5 + c4; c4 = 0 }
  if c7 == 0 { c7 = c6; c6 = c5; c5 = c4; c4 = 0; let temp = tid7; tid7 = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c6 == 0 { c6 = c5; c5 = c4; c4 = 0; let temp = tid6; tid6 = tid5; tid5 = tid4; tid4 = temp }
  if c5 == 0 { c5 = c4; c4 = 0; let temp = tid5; tid5 = tid4; tid4 = temp }
  if c11 == 0 { c11 = c10; c10 = c9; c9 = c8; c8 = 0; let temp = tid11; tid11 = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c11 == 0 { c11 = c10; c10 = c9; c9 = c8; c8 = 0; let temp = tid11; tid11 = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c11 == 0 { c11 = c10; c10 = c9; c9 = c8; c8 = 0; let temp = tid11; tid11 = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c10 == 0 { c10 = c9; c9 = c8; c8 = 0; let temp = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c10 == 0 { c10 = c9; c9 = c8; c8 = 0; let temp = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c9 == 0 { c9 = c8; c8 = 0; let temp = tid9; tid9 = tid8; tid8 = temp }
  if c11 == c10 { let temp = tid11; tid11 = tid10; tid10 = temp; c11 = c11 + c10; c10 = 0 }
  if c10 == c9 { let temp = tid10; tid10 = tid9; tid9 = temp; c10 = c10 + c9; c9 = 0 }
  if c9 == c8 { let temp = tid9; tid9 = tid8; tid8 = temp; c9 = c9 + c8; c8 = 0 }
  if c11 == 0 { c11 = c10; c10 = c9; c9 = c8; c8 = 0; let temp = tid11; tid11 = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c10 == 0 { c10 = c9; c9 = c8; c8 = 0; let temp = tid10; tid10 = tid9; tid9 = tid8; tid8 = temp }
  if c9 == 0 { c9 = c8; c8 = 0; let temp = tid9; tid9 = tid8; tid8 = temp }
  if c15 == 0 { c15 = c14; c14 = c13; c13 = c12; c12 = 0; let temp = tid15; tid15 = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c15 == 0 { c15 = c14; c14 = c13; c13 = c12; c12 = 0; let temp = tid15; tid15 = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c15 == 0 { c15 = c14; c14 = c13; c13 = c12; c12 = 0; let temp = tid15; tid15 = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c14 == 0 { c14 = c13; c13 = c12; c12 = 0; let temp = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c14 == 0 { c14 = c13; c13 = c12; c12 = 0; let temp = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c13 == 0 { c13 = c12; c12 = 0; let temp = tid13; tid13 = tid12; tid12 = temp }
  if c15 == c14 { let temp = tid15; tid15 = tid14; tid14 = temp; c15 = c15 + c14; c14 = 0 }
  if c14 == c13 { let temp = tid14; tid14 = tid13; tid13 = temp; c14 = c14 + c13; c13 = 0 }
  if c13 == c12 { let temp = tid13; tid13 = tid12; tid12 = temp; c13 = c13 + c12; c12 = 0 }
  if c15 == 0 { c15 = c14; c14 = c13; c13 = c12; c12 = 0; let temp = tid15; tid15 = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c14 == 0 { c14 = c13; c13 = c12; c12 = 0; let temp = tid14; tid14 = tid13; tid13 = tid12; tid12 = temp }
  if c13 == 0 { c13 = c12; c12 = 0; let temp = tid13; tid13 = tid12; tid12 = temp }
}
on goUp {
  if c0 == 0 { c0 = c4; c4 = c8; c8 = c12; c12 = 0; let temp = tid0; tid0 = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c0 == 0 { c0 = c4; c4 = c8; c8 = c12; c12 = 0; let temp = tid0; tid0 = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c0 == 0 { c0 = c4; c4 = c8; c8 = c12; c12 = 0; let temp = tid0; tid0 = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c4 == 0 { c4 = c8; c8 = c12; c12 = 0; let temp = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c4 == 0 { c4 = c8; c8 = c12; c12 = 0; let temp = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c8 == 0 { c8 = c12; c12 = 0; let temp = tid8; tid8 = tid12; tid12 = temp }
  if c0 == c4 { let temp = tid0; tid0 = tid4; tid4 = temp; c0 = c0 + c4; c4 = 0 }
  if c4 == c8 { let temp = tid4; tid4 = tid8; tid8 = temp; c4 = c4 + c8; c8 = 0 }
  if c8 == c12 { let temp = tid8; tid8 = tid12; tid12 = temp; c8 = c8 + c12; c12 = 0 }
  if c0 == 0 { c0 = c4; c4 = c8; c8 = c12; c12 = 0; let temp = tid0; tid0 = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c4 == 0 { c4 = c8; c8 = c12; c12 = 0; let temp = tid4; tid4 = tid8; tid8 = tid12; tid12 = temp }
  if c8 == 0 { c8 = c12; c12 = 0; let temp = tid8; tid8 = tid12; tid12 = temp }
  if c1 == 0 { c1 = c5; c5 = c9; c9 = c13; c13 = 0; let temp = tid1; tid1 = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c1 == 0 { c1 = c5; c5 = c9; c9 = c13; c13 = 0; let temp = tid1; tid1 = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c1 == 0 { c1 = c5; c5 = c9; c9 = c13; c13 = 0; let temp = tid1; tid1 = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c5 == 0 { c5 = c9; c9 = c13; c13 = 0; let temp = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c5 == 0 { c5 = c9; c9 = c13; c13 = 0; let temp = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c9 == 0 { c9 = c13; c13 = 0; let temp = tid9; tid9 = tid13; tid13 = temp }
  if c1 == c5 { let temp = tid1; tid1 = tid5; tid5 = temp; c1 = c1 + c5; c5 = 0 }
  if c5 == c9 { let temp = tid5; tid5 = tid9; tid9 = temp; c5 = c5 + c9; c9 = 0 }
  if c9 == c13 { let temp = tid9; tid9 = tid13; tid13 = temp; c9 = c9 + c13; c13 = 0 }
  if c1 == 0 { c1 = c5; c5 = c9; c9 = c13; c13 = 0; let temp = tid1; tid1 = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c5 == 0 { c5 = c9; c9 = c13; c13 = 0; let temp = tid5; tid5 = tid9; tid9 = tid13; tid13 = temp }
  if c9 == 0 { c9 = c13; c13 = 0; let temp = tid9; tid9 = tid13; tid13 = temp }
  if c2 == 0 { c2 = c6; c6 = c10; c10 = c14; c14 = 0; let temp = tid2; tid2 = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c2 == 0 { c2 = c6; c6 = c10; c10 = c14; c14 = 0; let temp = tid2; tid2 = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c2 == 0 { c2 = c6; c6 = c10; c10 = c14; c14 = 0; let temp = tid2; tid2 = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c6 == 0 { c6 = c10; c10 = c14; c14 = 0; let temp = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c6 == 0 { c6 = c10; c10 = c14; c14 = 0; let temp = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c10 == 0 { c10 = c14; c14 = 0; let temp = tid10; tid10 = tid14; tid14 = temp }
  if c2 == c6 { let temp = tid2; tid2 = tid6; tid6 = temp; c2 = c2 + c6; c6 = 0 }
  if c6 == c10 { let temp = tid6; tid6 = tid10; tid10 = temp; c6 = c6 + c10; c10 = 0 }
  if c10 == c14 { let temp = tid10; tid10 = tid14; tid14 = temp; c10 = c10 + c14; c14 = 0 }
  if c2 == 0 { c2 = c6; c6 = c10; c10 = c14; c14 = 0; let temp = tid2; tid2 = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c6 == 0 { c6 = c10; c10 = c14; c14 = 0; let temp = tid6; tid6 = tid10; tid10 = tid14; tid14 = temp }
  if c10 == 0 { c10 = c14; c14 = 0; let temp = tid10; tid10 = tid14; tid14 = temp }
  if c3 == 0 { c3 = c7; c7 = c11; c11 = c15; c15 = 0; let temp = tid3; tid3 = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c3 == 0 { c3 = c7; c7 = c11; c11 = c15; c15 = 0; let temp = tid3; tid3 = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c3 == 0 { c3 = c7; c7 = c11; c11 = c15; c15 = 0; let temp = tid3; tid3 = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c7 == 0 { c7 = c11; c11 = c15; c15 = 0; let temp = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c7 == 0 { c7 = c11; c11 = c15; c15 = 0; let temp = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c11 == 0 { c11 = c15; c15 = 0; let temp = tid11; tid11 = tid15; tid15 = temp }
  if c3 == c7 { let temp = tid3; tid3 = tid7; tid7 = temp; c3 = c3 + c7; c7 = 0 }
  if c7 == c11 { let temp = tid7; tid7 = tid11; tid11 = temp; c7 = c7 + c11; c11 = 0 }
  if c11 == c15 { let temp = tid11; tid11 = tid15; tid15 = temp; c11 = c11 + c15; c15 = 0 }
  if c3 == 0 { c3 = c7; c7 = c11; c11 = c15; c15 = 0; let temp = tid3; tid3 = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c7 == 0 { c7 = c11; c11 = c15; c15 = 0; let temp = tid7; tid7 = tid11; tid11 = tid15; tid15 = temp }
  if c11 == 0 { c11 = c15; c15 = 0; let temp = tid11; tid11 = tid15; tid15 = temp }
}
on goDown {
  if c12 == 0 { c12 = c8; c8 = c4; c4 = c0; c0 = 0; let temp = tid12; tid12 = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c12 == 0 { c12 = c8; c8 = c4; c4 = c0; c0 = 0; let temp = tid12; tid12 = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c12 == 0 { c12 = c8; c8 = c4; c4 = c0; c0 = 0; let temp = tid12; tid12 = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c8 == 0 { c8 = c4; c4 = c0; c0 = 0; let temp = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c8 == 0 { c8 = c4; c4 = c0; c0 = 0; let temp = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c4 == 0 { c4 = c0; c0 = 0; let temp = tid4; tid4 = tid0; tid0 = temp }
  if c12 == c8 { let temp = tid12; tid12 = tid8; tid8 = temp; c12 = c12 + c8; c8 = 0 }
  if c8 == c4 { let temp = tid8; tid8 = tid4; tid4 = temp; c8 = c8 + c4; c4 = 0 }
  if c4 == c0 { let temp = tid4; tid4 = tid0; tid0 = temp; c4 = c4 + c0; c0 = 0 }
  if c12 == 0 { c12 = c8; c8 = c4; c4 = c0; c0 = 0; let temp = tid12; tid12 = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c8 == 0 { c8 = c4; c4 = c0; c0 = 0; let temp = tid8; tid8 = tid4; tid4 = tid0; tid0 = temp }
  if c4 == 0 { c4 = c0; c0 = 0; let temp = tid4; tid4 = tid0; tid0 = temp }
  if c13 == 0 { c13 = c9; c9 = c5; c5 = c1; c1 = 0; let temp = tid13; tid13 = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c13 == 0 { c13 = c9; c9 = c5; c5 = c1; c1 = 0; let temp = tid13; tid13 = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c13 == 0 { c13 = c9; c9 = c5; c5 = c1; c1 = 0; let temp = tid13; tid13 = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c9 == 0 { c9 = c5; c5 = c1; c1 = 0; let temp = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c9 == 0 { c9 = c5; c5 = c1; c1 = 0; let temp = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c5 == 0 { c5 = c1; c1 = 0; let temp = tid5; tid5 = tid1; tid1 = temp }
  if c13 == c9 { let temp = tid13; tid13 = tid9; tid9 = temp; c13 = c13 + c9; c9 = 0 }
  if c9 == c5 { let temp = tid9; tid9 = tid5; tid5 = temp; c9 = c9 + c5; c5 = 0 }
  if c5 == c1 { let temp = tid5; tid5 = tid1; tid1 = temp; c5 = c5 + c1; c1 = 0 }
  if c13 == 0 { c13 = c9; c9 = c5; c5 = c1; c1 = 0; let temp = tid13; tid13 = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c9 == 0 { c9 = c5; c5 = c1; c1 = 0; let temp = tid9; tid9 = tid5; tid5 = tid1; tid1 = temp }
  if c5 == 0 { c5 = c1; c1 = 0; let temp = tid5; tid5 = tid1; tid1 = temp }
  if c14 == 0 { c14 = c10; c10 = c6; c6 = c2; c2 = 0; let temp = tid14; tid14 = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c14 == 0 { c14 = c10; c10 = c6; c6 = c2; c2 = 0; let temp = tid14; tid14 = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c14 == 0 { c14 = c10; c10 = c6; c6 = c2; c2 = 0; let temp = tid14; tid14 = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c10 == 0 { c10 = c6; c6 = c2; c2 = 0; let temp = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c10 == 0 { c10 = c6; c6 = c2; c2 = 0; let temp = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c6 == 0 { c6 = c2; c2 = 0; let temp = tid6; tid6 = tid2; tid2 = temp }
  if c14 == c10 { let temp = tid14; tid14 = tid10; tid10 = temp; c14 = c14 + c10; c10 = 0 }
  if c10 == c6 { let temp = tid10; tid10 = tid6; tid6 = temp; c10 = c10 + c6; c6 = 0 }
  if c6 == c2 { let temp = tid6; tid6 = tid2; tid2 = temp; c6 = c6 + c2; c2 = 0 }
  if c14 == 0 { c14 = c10; c10 = c6; c6 = c2; c2 = 0; let temp = tid14; tid14 = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c10 == 0 { c10 = c6; c6 = c2; c2 = 0; let temp = tid10; tid10 = tid6; tid6 = tid2; tid2 = temp }
  if c6 == 0 { c6 = c2; c2 = 0; let temp = tid6; tid6 = tid2; tid2 = temp }
  if c15 == 0 { c15 = c11; c11 = c7; c7 = c3; c3 = 0; let temp = tid15; tid15 = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c15 == 0 { c15 = c11; c11 = c7; c7 = c3; c3 = 0; let temp = tid15; tid15 = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c15 == 0 { c15 = c11; c11 = c7; c7 = c3; c3 = 0; let temp = tid15; tid15 = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c11 == 0 { c11 = c7; c7 = c3; c3 = 0; let temp = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c11 == 0 { c11 = c7; c7 = c3; c3 = 0; let temp = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c7 == 0 { c7 = c3; c3 = 0; let temp = tid7; tid7 = tid3; tid3 = temp }
  if c15 == c11 { let temp = tid15; tid15 = tid11; tid11 = temp; c15 = c15 + c11; c11 = 0 }
  if c11 == c7 { let temp = tid11; tid11 = tid7; tid7 = temp; c11 = c11 + c7; c7 = 0 }
  if c7 == c3 { let temp = tid7; tid7 = tid3; tid3 = temp; c7 = c7 + c3; c3 = 0 }
  if c15 == 0 { c15 = c11; c11 = c7; c7 = c3; c3 = 0; let temp = tid15; tid15 = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c11 == 0 { c11 = c7; c7 = c3; c3 = 0; let temp = tid11; tid11 = tid7; tid7 = tid3; tid3 = temp }
  if c7 == 0 { c7 = c3; c3 = 0; let temp = tid7; tid7 = tid3; tid3 = temp }
}

// Shared: Random tile + render (exec-unioned from all handlers above)
let moved = c0 != c0.prev || c1 != c1.prev || c2 != c2.prev || c3 != c3.prev || c4 != c4.prev || c5 != c5.prev || c6 != c6.prev || c7 != c7.prev || c8 != c8.prev || c9 != c9.prev || c10 != c10.prev || c11 != c11.prev || c12 != c12.prev || c13 != c13.prev || c14 != c14.prev || c15 != c15.prev

// Top-level aggregates published as outputs so external wiring can read them.
let score = c0 + c1 + c2 + c3 + c4 + c5 + c6 + c7 + c8 + c9 + c10 + c11 + c12 + c13 + c14 + c15
let emptyCount = (if c0 == 0 then 1 else 0) + (if c1 == 0 then 1 else 0) + (if c2 == 0 then 1 else 0) + (if c3 == 0 then 1 else 0) + (if c4 == 0 then 1 else 0) + (if c5 == 0 then 1 else 0) + (if c6 == 0 then 1 else 0) + (if c7 == 0 then 1 else 0) + (if c8 == 0 then 1 else 0) + (if c9 == 0 then 1 else 0) + (if c10 == 0 then 1 else 0) + (if c11 == 0 then 1 else 0) + (if c12 == 0 then 1 else 0) + (if c13 == 0 then 1 else 0) + (if c14 == 0 then 1 else 0) + (if c15 == 0 then 1 else 0)
out score = score
out emptyCount = emptyCount

if running && moved {
  let b0 = 0
  let b1 = b0 + if c0 == 0 then 1 else 0
  let b2 = b1 + if c1 == 0 then 1 else 0
  let b3 = b2 + if c2 == 0 then 1 else 0
  let b4 = b3 + if c3 == 0 then 1 else 0
  let b5 = b4 + if c4 == 0 then 1 else 0
  let b6 = b5 + if c5 == 0 then 1 else 0
  let b7 = b6 + if c6 == 0 then 1 else 0
  let b8 = b7 + if c7 == 0 then 1 else 0
  let b9 = b8 + if c8 == 0 then 1 else 0
  let b10 = b9 + if c9 == 0 then 1 else 0
  let b11 = b10 + if c10 == 0 then 1 else 0
  let b12 = b11 + if c11 == 0 then 1 else 0
  let b13 = b12 + if c12 == 0 then 1 else 0
  let b14 = b13 + if c13 == 0 then 1 else 0
  let b15 = b14 + if c14 == 0 then 1 else 0
  let r = Random(0, emptyCount - 1)
  // Compute placement flags BEFORE placement so transition logic can skip them.
  let p0 = c0 == 0 && r == b0
  let p1 = c1 == 0 && r == b1
  let p2 = c2 == 0 && r == b2
  let p3 = c3 == 0 && r == b3
  let p4 = c4 == 0 && r == b4
  let p5 = c5 == 0 && r == b5
  let p6 = c6 == 0 && r == b6
  let p7 = c7 == 0 && r == b7
  let p8 = c8 == 0 && r == b8
  let p9 = c9 == 0 && r == b9
  let p10 = c10 == 0 && r == b10
  let p11 = c11 == 0 && r == b11
  let p12 = c12 == 0 && r == b12
  let p13 = c13 == 0 && r == b13
  let p14 = c14 == 0 && r == b14
  let p15 = c15 == 0 && r == b15
  if p0 { c0 = 2 }
  if p1 { c1 = 2 }
  if p2 { c2 = 2 }
  if p3 { c3 = 2 }
  if p4 { c4 = 2 }
  if p5 { c5 = 2 }
  if p6 { c6 = 2 }
  if p7 { c7 = 2 }
  if p8 { c8 = 2 }
  if p9 { c9 = 2 }
  if p10 { c10 = 2 }
  if p11 { c11 = 2 }
  if p12 { c12 = 2 }
  if p13 { c13 = 2 }
  if p14 { c14 = 2 }
  if p15 { c15 = 2 }
  // Background board
  DisplayText(ctrl, "<color=\"2a2a2a\"><icon>stop</></>", positionX = 0.0, positionY = 0.0, lifetime = 10.0, fontSize = 256, outlineSize = 0, scaleX = 2.5, scaleY = 2.5, textId = 17)
  // Title + score
  DisplayText(ctrl, "<color=\"ffffff\">2048</>", positionX = -240.0, positionY = -280.0, justify = "Left", lifetime = 10.0, fontSize = 60, textId = 18)
  DisplayText(ctrl, "<color=\"bbada0\">${score}</>", positionX = 240.0, positionY = -280.0, justify = "Right", lifetime = 10.0, fontSize = 60, textId = 19)
  // Per-cell transition: animate only cells that changed.
  let t0 = if p0 || c0 == 0 || c0 == c0.prev then 0.0 else 0.15
  let t1 = if p1 || c1 == 0 || c1 == c1.prev then 0.0 else 0.15
  let t2 = if p2 || c2 == 0 || c2 == c2.prev then 0.0 else 0.15
  let t3 = if p3 || c3 == 0 || c3 == c3.prev then 0.0 else 0.15
  let t4 = if p4 || c4 == 0 || c4 == c4.prev then 0.0 else 0.15
  let t5 = if p5 || c5 == 0 || c5 == c5.prev then 0.0 else 0.15
  let t6 = if p6 || c6 == 0 || c6 == c6.prev then 0.0 else 0.15
  let t7 = if p7 || c7 == 0 || c7 == c7.prev then 0.0 else 0.15
  let t8 = if p8 || c8 == 0 || c8 == c8.prev then 0.0 else 0.15
  let t9 = if p9 || c9 == 0 || c9 == c9.prev then 0.0 else 0.15
  let t10 = if p10 || c10 == 0 || c10 == c10.prev then 0.0 else 0.15
  let t11 = if p11 || c11 == 0 || c11 == c11.prev then 0.0 else 0.15
  let t12 = if p12 || c12 == 0 || c12 == c12.prev then 0.0 else 0.15
  let t13 = if p13 || c13 == 0 || c13 == c13.prev then 0.0 else 0.15
  let t14 = if p14 || c14 == 0 || c14 == c14.prev then 0.0 else 0.15
  let t15 = if p15 || c15 == 0 || c15 == c15.prev then 0.0 else 0.15
  // Per-cell colored render (value-based bucket via nested Fmt)
  let b0 = if c0 <= 4 then 0 else if c0 <= 16 then 1 else if c0 <= 64 then 2 else if c0 <= 256 then 3 else if c0 <= 1024 then 4 else if c0 <= 2048 then 5 else 6
  let col0 = Fmt("{" .. b0 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt0 = "<color=\"${col0}\">${c0}</>"
  DisplayText(ctrl, txt0, positionX = -180.0, positionY = -180.0, lifetime = 10.0, fontSize = 40, transition = t0, textId = tid0)
  let b1 = if c1 <= 4 then 0 else if c1 <= 16 then 1 else if c1 <= 64 then 2 else if c1 <= 256 then 3 else if c1 <= 1024 then 4 else if c1 <= 2048 then 5 else 6
  let col1 = Fmt("{" .. b1 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt1 = "<color=\"${col1}\">${c1}</>"
  DisplayText(ctrl, txt1, positionX = -60.0, positionY = -180.0, lifetime = 10.0, fontSize = 40, transition = t1, textId = tid1)
  let b2 = if c2 <= 4 then 0 else if c2 <= 16 then 1 else if c2 <= 64 then 2 else if c2 <= 256 then 3 else if c2 <= 1024 then 4 else if c2 <= 2048 then 5 else 6
  let col2 = Fmt("{" .. b2 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt2 = "<color=\"${col2}\">${c2}</>"
  DisplayText(ctrl, txt2, positionX = 60.0, positionY = -180.0, lifetime = 10.0, fontSize = 40, transition = t2, textId = tid2)
  let b3 = if c3 <= 4 then 0 else if c3 <= 16 then 1 else if c3 <= 64 then 2 else if c3 <= 256 then 3 else if c3 <= 1024 then 4 else if c3 <= 2048 then 5 else 6
  let col3 = Fmt("{" .. b3 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt3 = "<color=\"${col3}\">${c3}</>"
  DisplayText(ctrl, txt3, positionX = 180.0, positionY = -180.0, lifetime = 10.0, fontSize = 40, transition = t3, textId = tid3)
  let b4 = if c4 <= 4 then 0 else if c4 <= 16 then 1 else if c4 <= 64 then 2 else if c4 <= 256 then 3 else if c4 <= 1024 then 4 else if c4 <= 2048 then 5 else 6
  let col4 = Fmt("{" .. b4 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt4 = "<color=\"${col4}\">${c4}</>"
  DisplayText(ctrl, txt4, positionX = -180.0, positionY = -60.0, lifetime = 10.0, fontSize = 40, transition = t4, textId = tid4)
  let b5 = if c5 <= 4 then 0 else if c5 <= 16 then 1 else if c5 <= 64 then 2 else if c5 <= 256 then 3 else if c5 <= 1024 then 4 else if c5 <= 2048 then 5 else 6
  let col5 = Fmt("{" .. b5 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt5 = "<color=\"${col5}\">${c5}</>"
  DisplayText(ctrl, txt5, positionX = -60.0, positionY = -60.0, lifetime = 10.0, fontSize = 40, transition = t5, textId = tid5)
  let b6 = if c6 <= 4 then 0 else if c6 <= 16 then 1 else if c6 <= 64 then 2 else if c6 <= 256 then 3 else if c6 <= 1024 then 4 else if c6 <= 2048 then 5 else 6
  let col6 = Fmt("{" .. b6 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt6 = "<color=\"${col6}\">${c6}</>"
  DisplayText(ctrl, txt6, positionX = 60.0, positionY = -60.0, lifetime = 10.0, fontSize = 40, transition = t6, textId = tid6)
  let b7 = if c7 <= 4 then 0 else if c7 <= 16 then 1 else if c7 <= 64 then 2 else if c7 <= 256 then 3 else if c7 <= 1024 then 4 else if c7 <= 2048 then 5 else 6
  let col7 = Fmt("{" .. b7 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt7 = "<color=\"${col7}\">${c7}</>"
  DisplayText(ctrl, txt7, positionX = 180.0, positionY = -60.0, lifetime = 10.0, fontSize = 40, transition = t7, textId = tid7)
  let b8 = if c8 <= 4 then 0 else if c8 <= 16 then 1 else if c8 <= 64 then 2 else if c8 <= 256 then 3 else if c8 <= 1024 then 4 else if c8 <= 2048 then 5 else 6
  let col8 = Fmt("{" .. b8 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt8 = "<color=\"${col8}\">${c8}</>"
  DisplayText(ctrl, txt8, positionX = -180.0, positionY = 60.0, lifetime = 10.0, fontSize = 40, transition = t8, textId = tid8)
  let b9 = if c9 <= 4 then 0 else if c9 <= 16 then 1 else if c9 <= 64 then 2 else if c9 <= 256 then 3 else if c9 <= 1024 then 4 else if c9 <= 2048 then 5 else 6
  let col9 = Fmt("{" .. b9 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt9 = "<color=\"${col9}\">${c9}</>"
  DisplayText(ctrl, txt9, positionX = -60.0, positionY = 60.0, lifetime = 10.0, fontSize = 40, transition = t9, textId = tid9)
  let b10 = if c10 <= 4 then 0 else if c10 <= 16 then 1 else if c10 <= 64 then 2 else if c10 <= 256 then 3 else if c10 <= 1024 then 4 else if c10 <= 2048 then 5 else 6
  let col10 = Fmt("{" .. b10 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt10 = "<color=\"${col10}\">${c10}</>"
  DisplayText(ctrl, txt10, positionX = 60.0, positionY = 60.0, lifetime = 10.0, fontSize = 40, transition = t10, textId = tid10)
  let b11 = if c11 <= 4 then 0 else if c11 <= 16 then 1 else if c11 <= 64 then 2 else if c11 <= 256 then 3 else if c11 <= 1024 then 4 else if c11 <= 2048 then 5 else 6
  let col11 = Fmt("{" .. b11 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt11 = "<color=\"${col11}\">${c11}</>"
  DisplayText(ctrl, txt11, positionX = 180.0, positionY = 60.0, lifetime = 10.0, fontSize = 40, transition = t11, textId = tid11)
  let b12 = if c12 <= 4 then 0 else if c12 <= 16 then 1 else if c12 <= 64 then 2 else if c12 <= 256 then 3 else if c12 <= 1024 then 4 else if c12 <= 2048 then 5 else 6
  let col12 = Fmt("{" .. b12 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt12 = "<color=\"${col12}\">${c12}</>"
  DisplayText(ctrl, txt12, positionX = -180.0, positionY = 180.0, lifetime = 10.0, fontSize = 40, transition = t12, textId = tid12)
  let b13 = if c13 <= 4 then 0 else if c13 <= 16 then 1 else if c13 <= 64 then 2 else if c13 <= 256 then 3 else if c13 <= 1024 then 4 else if c13 <= 2048 then 5 else 6
  let col13 = Fmt("{" .. b13 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt13 = "<color=\"${col13}\">${c13}</>"
  DisplayText(ctrl, txt13, positionX = -60.0, positionY = 180.0, lifetime = 10.0, fontSize = 40, transition = t13, textId = tid13)
  let b14 = if c14 <= 4 then 0 else if c14 <= 16 then 1 else if c14 <= 64 then 2 else if c14 <= 256 then 3 else if c14 <= 1024 then 4 else if c14 <= 2048 then 5 else 6
  let col14 = Fmt("{" .. b14 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt14 = "<color=\"${col14}\">${c14}</>"
  DisplayText(ctrl, txt14, positionX = 60.0, positionY = 180.0, lifetime = 10.0, fontSize = 40, transition = t14, textId = tid14)
  let b15 = if c15 <= 4 then 0 else if c15 <= 16 then 1 else if c15 <= 64 then 2 else if c15 <= 256 then 3 else if c15 <= 1024 then 4 else if c15 <= 2048 then 5 else 6
  let col15 = Fmt("{" .. b15 .. "}", "eee4da", "f2b179", "f65e3b", "edcf72", "edc850", "edc22e", "3c3a32")
  let txt15 = "<color=\"${col15}\">${c15}</>"
  DisplayText(ctrl, txt15, positionX = 180.0, positionY = 180.0, lifetime = 10.0, fontSize = 40, transition = t15, textId = tid15)
}
