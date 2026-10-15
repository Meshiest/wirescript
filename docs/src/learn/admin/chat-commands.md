# Chat Commands

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Admin Gates](admin-gates.md), [Taking Text Apart](../strings/taking-text-apart.md), [Safe Text](../strings/safe-text.md)

**Next steps:** [Every Player](every-player.md)

</div>
<!-- /learn-links -->

A chat command is a word typed into chat after a `/`, like `/roll`. A circuit
can add its own, read what the player typed after it, and answer them. This
lesson builds a dice roll, a lamp set from chat, a private message, and a
nickname announcement.

Needs admin to load: every command uses **Chat Command Event**. `/pm` also
uses **Find Player by Name**, and `/nick` **Broadcast Chat Message**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [A setting and a number](#a-setting-and-a-number)
- [One player, or everyone](#one-player-or-everyone)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

## Code

```wirescript name=a6-roll admin
on ChatCommand("roll", "Roll a six-sided die") -> (who, arguments) {
  let n = Random(1, 6)
  who.ShowChatMessage("You rolled ${n}")
}
```

- `on ChatCommand("roll", ...)` adds `/roll` to chat. The second text is the
  help line the game shows for it.
- `Random(1, 6)` picks a whole number from 1 to 6, different each time.
- `-> (who, arguments)` names what the event gives: `who` is the player who
  typed it, and `arguments` is the rest of the line after `/roll `.
- `who.ShowChatMessage` answers only that player.

A player types `/roll`. Who sees the result? Another types `/roll 20`. Do
they get a twenty-sided die?

## Run

1. Compile the chip and paste it in-game. Nothing needs wiring: the command
   is the input.
2. Type `/roll` in chat. You get a number from 1 to 6, and nobody else sees
   it.
3. Type `/roll 20`. Still 1 to 6: the chip ignores `arguments`.

## Under the Hood

Open the chip. **Chat Command Event** has the command name and help text in
its settings and no inputs. Its outputs are **Exec Out**, **Player State**
(the player) and **Arguments**. The chain runs through **Random** and **Show
Chat Message**, with a **Format String** building the text.

Arguments are always text, even when they look like a number.
[Taking Text Apart](../strings/taking-text-apart.md) covers reading them. Here `/roll 20` asks for
a bigger die, and anything that is not a number gives the usual six:

```wirescript name=a6-roll-sides admin
on ChatCommand("roll", "Roll a die: /roll or /roll 20") -> (who, arguments) {
  let asked = arguments.Trim().ParseInt()
  let sides = if asked.Success && asked.Value >= 2 then asked.Value else 6
  let n = Random(1, sides)
  who.ShowChatMessage("You rolled ${n} on a d${sides}")
}
```

## A setting and a number

`/lamp brightness 20` sets one setting of a lamp. `Split(" ")` cuts the
arguments at the first space: the setting name on the left, the number on the
right. Anything that does not match gets the usage line back.

```wirescript name=a6-lamp admin
const MAX_BRIGHTNESS = 100.0
const MAX_RADIUS = 2000.0
const USAGE = "Usage: /lamp brightness 0 to 100, or /lamp radius 0 to 2000"

var level: float = 10.0
var reach: float = 500.0
@right out brightness = level.Value
@right out radius = reach.Value

on ChatCommand("lamp", "Set the lamp: /lamp brightness 20") -> (who, arguments) {
  let parts = arguments.Trim().Split(" ")
  let setting = parts.Left.ToLower()
  let number = parts.Right.Trim().ParseNumber()
  if !parts.Found || !number.Success || number.Value < 0.0 {
    who.ShowChatMessage(USAGE)
  } else if setting == "brightness" && number.Value <= MAX_BRIGHTNESS {
    level = number.Value
    who.ShowChatMessage("Brightness is now ${number.Value}")
  } else if setting == "radius" && number.Value <= MAX_RADIUS {
    reach = number.Value
    who.ShowChatMessage("Radius is now ${number.Value}")
  } else {
    who.ShowChatMessage(USAGE)
  }
}
```

Wire `brightness` and `radius` to a **Point Light**'s **Brightness** and
**Radius**. `ToLower()` lets `/lamp Brightness 20` work too.

## One player, or everyone

`/pm name message` whispers to another player. `FindPlayer` looks a player up
by name or user id. It is admin too, and [Every Player](every-player.md)
shows another way to search.

```wirescript name=a6-pm admin
import { escape, sanitize } from "sanitize"

const MAX_LEN = 200

on ChatCommand("pm", "Whisper to one player: /pm name message") -> (who, arguments) {
  let parts = arguments.Trim().Split(" ")
  if !parts.Found {
    who.ShowChatMessage("Usage: /pm name message")
    return
  }
  let to = FindPlayer(parts.Left)
  if to == null {
    who.ShowChatMessage("Nobody is called ${sanitize(parts.Left)}")
    return
  }
  let text = sanitize(parts.Right, MAX_LEN)
  to.ShowChatMessage("[from ${escape(who.GetDisplayName())}] ${text}")
  who.ShowChatMessage("[to ${escape(to.GetDisplayName())}] ${text}")
}
```

When nobody matches, `to` is empty, and `to == null` is the check
([Object References](../references/object-references.md)). `return` ends the handler there, so nothing after it runs.
Every piece of text a player typed goes through `sanitize` before it is shown, and every name through `escape`, as in
[Safe Text](../strings/safe-text.md). A message is text another player chose,
and it is shown to someone else.

`/nick name` tells the whole server, with `BroadcastChatMessage`:

```wirescript name=a6-nick admin
import { escape, sanitize } from "sanitize"

on ChatCommand("nick", "Announce a nickname: /nick name") -> (who, arguments) {
  let nick = sanitize(arguments)
  if nick == "" {
    who.ShowChatMessage("Usage: /nick name")
    return
  }
  BroadcastChatMessage("${escape(who.GetDisplayName())} is now known as ${nick}")
}
```

## Modify

1. Add `/flip`: answer "Heads" or "Tails" with `Random(0, 1)`.
2. Add `/roll2`: roll two dice, and answer with both rolls and the total.
   Let `/roll2 20` set the sides of both, as `/roll 20` does.
3. Make `/pm` tell the receiver how to answer: add "(reply with /pm name
   message)" after the text, with the sender's name filled in.

## Make

Make a `/vote yes` and `/vote no` poll. A sign shows the two tallies, and
`/vote reset` clears the poll. Anything else after `/vote` gets a usage line
back. Then add a `/vote close` that stops counting until the next reset.

```wiring
chip sign -> decal Text
```

## Good to Know

- A command fires for whoever types it, anywhere in the world. Check the
  player yourself if only some should be able to use it.
  [Moderation](moderation.md) shows how.
- `arguments` is the rest of the line as the player typed it: extra spaces,
  any case, any symbols. Trim it, and escape it before showing it.

See [ChatCommand](../../builtins.md#chatcommand-event) and
[FindPlayer](../../builtins.md#misc-pure--exec) in the reference.

Next: [Every Player](every-player.md).
