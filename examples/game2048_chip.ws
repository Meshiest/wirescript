// 2048 Game — wirescript implementation

in running: bool
in player: character

var ctrl: controller
var char: character

chip {
  var c0: int = 0; var c1: int = 0; var c2: int = 0; var c3: int = 0
  var c4: int = 0; var c5: int = 0; var c6: int = 0; var c7: int = 0
  var c8: int = 0; var c9: int = 0; var c10: int = 0; var c11: int = 0
  var c12: int = 0; var c13: int = 0; var c14: int = 0; var c15: int = 0
}

chip {
  var tid0: int = 1; var tid1: int = 2; var tid2: int = 3; var tid3: int = 4
  var tid4: int = 5; var tid5: int = 6; var tid6: int = 7; var tid7: int = 8
  var tid8: int = 9; var tid9: int = 10; var tid10: int = 11; var tid11: int = 12
  var tid12: int = 13; var tid13: int = 14; var tid14: int = 15; var tid15: int = 16
}

chip let input = InputReader(char),
goUp = running && input.Forward > 0.5,
goDown = running && input.Forward < -0.5,
goRight = running && input.Right > 0.5,
goLeft = running && input.Right < -0.5

on running {
  ctrl = ControllerOf(player)
  char = player
  chip {
    c0 = 0; c1 = 0; c2 = 0; c3 = 0
    c4 = 0; c5 = 0; c6 = 0; c7 = 0
    c8 = 0; c9 = 0; c10 = 0; c11 = 0
    c12 = 0; c13 = 0; c14 = 0; c15 = 0
    tid0 = 1; tid1 = 2; tid2 = 3; tid3 = 4
    tid4 = 5; tid5 = 6; tid6 = 7; tid7 = 8
    tid8 = 9; tid9 = 10; tid10 = 11; tid11 = 12
    tid12 = 13; tid13 = 14; tid14 = 15; tid15 = 16
  }
  chip {
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
}

chip on !running {
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

mod slide(a: *int, b: *int, c: *int, d: *int, ta: *int, tb: *int, tc: *int, td: *int) {
  if a == 0 { a = b; b = c; c = d; d = 0; let t = ta; ta = tb; tb = tc; tc = td; td = t }
  if a == 0 { a = b; b = c; c = d; d = 0; let t = ta; ta = tb; tb = tc; tc = td; td = t }
  if a == 0 { a = b; b = c; c = d; d = 0; let t = ta; ta = tb; tb = tc; tc = td; td = t }
  if b == 0 { b = c; c = d; d = 0; let t = tb; tb = tc; tc = td; td = t }
  if b == 0 { b = c; c = d; d = 0; let t = tb; tb = tc; tc = td; td = t }
  if c == 0 { c = d; d = 0; let t = tc; tc = td; td = t }
  if a == b { let t = ta; ta = tb; tb = t; a = a + b; b = 0 }
  if b == c { let t = tb; tb = tc; tc = t; b = b + c; c = 0 }
  if c == d { let t = tc; tc = td; td = t; c = c + d; d = 0 }
  if a == 0 { a = b; b = c; c = d; d = 0; let t = ta; ta = tb; tb = tc; tc = td; td = t }
  if b == 0 { b = c; c = d; d = 0; let t = tb; tb = tc; tc = td; td = t }
  if c == 0 { c = d; d = 0; let t = tc; tc = td; td = t }
}

chip on goLeft {
  slide(c0, c1, c2, c3, tid0, tid1, tid2, tid3)
  slide(c4, c5, c6, c7, tid4, tid5, tid6, tid7)
  slide(c8, c9, c10, c11, tid8, tid9, tid10, tid11)
  slide(c12, c13, c14, c15, tid12, tid13, tid14, tid15)
}
chip on goRight {
  slide(c3, c2, c1, c0, tid3, tid2, tid1, tid0)
  slide(c7, c6, c5, c4, tid7, tid6, tid5, tid4)
  slide(c11, c10, c9, c8, tid11, tid10, tid9, tid8)
  slide(c15, c14, c13, c12, tid15, tid14, tid13, tid12)
}
chip on goUp {
  slide(c0, c4, c8, c12, tid0, tid4, tid8, tid12)
  slide(c1, c5, c9, c13, tid1, tid5, tid9, tid13)
  slide(c2, c6, c10, c14, tid2, tid6, tid10, tid14)
  slide(c3, c7, c11, c15, tid3, tid7, tid11, tid15)
}
chip on goDown {
  slide(c12, c8, c4, c0, tid12, tid8, tid4, tid0)
  slide(c13, c9, c5, c1, tid13, tid9, tid5, tid1)
  slide(c14, c10, c6, c2, tid14, tid10, tid6, tid2)
  slide(c15, c11, c7, c3, tid15, tid11, tid7, tid3)
}

chip let moved = c0 != c0.prev || c1 != c1.prev || c2 != c2.prev || c3 != c3.prev || c4 != c4.prev || c5 != c5.prev || c6 != c6.prev || c7 != c7.prev || c8 != c8.prev || c9 != c9.prev || c10 != c10.prev || c11 != c11.prev || c12 != c12.prev || c13 != c13.prev || c14 != c14.prev || c15 != c15.prev
chip let score = c0 + c1 + c2 + c3 + c4 + c5 + c6 + c7 + c8 + c9 + c10 + c11 + c12 + c13 + c14 + c15
chip let emptyCount = (if c0 == 0 then 1 else 0) + (if c1 == 0 then 1 else 0) + (if c2 == 0 then 1 else 0) + (if c3 == 0 then 1 else 0) + (if c4 == 0 then 1 else 0) + (if c5 == 0 then 1 else 0) + (if c6 == 0 then 1 else 0) + (if c7 == 0 then 1 else 0) + (if c8 == 0 then 1 else 0) + (if c9 == 0 then 1 else 0) + (if c10 == 0 then 1 else 0) + (if c11 == 0 then 1 else 0) + (if c12 == 0 then 1 else 0) + (if c13 == 0 then 1 else 0) + (if c14 == 0 then 1 else 0) + (if c15 == 0 then 1 else 0)

out score = score
out emptyCount = emptyCount

if running && moved {
  chip {
    chip {
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
    }
    let r = Random(0, emptyCount - 1)
    chip {
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
    }
    chip {
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
    }
  }
  DisplayText(ctrl, '<color="2a2a2a"><icon>stop</></>', positionX = 0.0, positionY = 0.0, lifetime = 10.0, fontSize = 256, outlineSize = 0, scaleX = 2.5, scaleY = 2.5, textId = 17)
  DisplayText(ctrl, '<color="ffffff">2048</>', positionX = -240.0, positionY = -280.0, justify = "Left", lifetime = 10.0, fontSize = 60, textId = 18)
  DisplayText(ctrl, '<color="bbada0">${score}</>', positionX = 240.0, positionY = -280.0, justify = "Right", lifetime = 10.0, fontSize = 60, textId = 19)

  mod renderCell(c: *int, tid: *int, p: bool, px: float, py: float) {
    let t = if p || c == 0 || c == c.prev then 0.0 else 0.15
    let bucket = if c <= 4 then 0 else if c <= 16 then 1 else if c <= 64 then 2 else if c <= 256 then 3 else if c <= 1024 then 4 else if c <= 2048 then 5 else 6
    let col = Fmt('{' .. bucket .. '}', 'eee4da', 'f2b179', 'f65e3b', 'edcf72', 'edc850', 'edc22e', '3c3a32')
    let txt = '<color="${col}">${c}</>'
    DisplayText(ctrl, txt, positionX = px, positionY = py, lifetime = 10.0, fontSize = 40, transition = t, textId = tid)
  }

  chip {
    renderCell(c0, tid0, p0, -180.0, -180.0)
    renderCell(c1, tid1, p1, -60.0, -180.0)
    renderCell(c2, tid2, p2, 60.0, -180.0)
    renderCell(c3, tid3, p3, 180.0, -180.0)
    renderCell(c4, tid4, p4, -180.0, -60.0)
    renderCell(c5, tid5, p5, -60.0, -60.0)
    renderCell(c6, tid6, p6, 60.0, -60.0)
    renderCell(c7, tid7, p7, 180.0, -60.0)
    renderCell(c8, tid8, p8, -180.0, 60.0)
    renderCell(c9, tid9, p9, -60.0, 60.0)
    renderCell(c10, tid10, p10, 60.0, 60.0)
    renderCell(c11, tid11, p11, 180.0, 60.0)
    renderCell(c12, tid12, p12, -180.0, 180.0)
    renderCell(c13, tid13, p13, -60.0, 180.0)
    renderCell(c14, tid14, p14, 60.0, 180.0)
    renderCell(c15, tid15, p15, 180.0, 180.0)
  }
}
