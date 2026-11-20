# vnengine DSL reference (`.vn`)

The `.vn` language is a line-oriented, indentation-significant scripting
language for visual novels, inspired by Ren'Py. Every fenced `vn` block in
this document is validated by the engine test suite.

```vn
# A comment. Comments run to the end of the line.

define e = Character("Eileen", color = "#ff4040")
default score = 0

label start:
    scene bg school with dissolve
    show eileen happy at center
    e "Hello, world!"
    "This line is narration — no speaker."
    end
```

A script is a list of **top-level declarations** (`define`, `default`,
`def`, `transform`, `image`, `screen`, `achievement`, `label`). All
story flow happens inside `label` blocks; execution starts at
`label start:`.

---

## Values and expressions

```vn
label start:
    $ i = 42                  # Int
    $ f = 3.5                 # Float
    $ s = "text"              # Str ('single' quotes work too)
    $ b = True                # Bool: True / False
    $ n = None                # None
    $ l = [1, "two", 3.0]     # List
    $ d = {"name": "Eileen", level: 2}   # Dict (ident or string keys)

    $ item = l[0]             # index; negative allowed: l[-1]
    $ part = l[1:3]           # Python-style slices
    $ tail = s[:-1]
    $ who = d["name"]
    $ lvl = d.level           # attribute access on dicts
    $ shout = "ab" * 3 + "!"  # "ababab!"
    $ ok = "key" in bag and "rope" not in bag
    $ same = l is l           # identity: `is` / `is not`
```

Operators: `+ - * / %`, comparisons `== != < <= > >=`, `and or not`,
`in / not in`, `is / is not`. String repeat: `"ab" * 3`.

### Built-in functions

```vn
label start:
    $ r = random()            # float in [0, 1)
    $ n = randint(1, 6)       # inclusive dice roll
    $ s = srand(1234)         # reseed the PRNG (serialized in saves)
    $ ln = len([1, 2, 3])
    $ t = str(42)
    $ x = int("7")
    $ c = choice(["a", "b"])
    $ trio = sample([1, 2, 3, 4], 3)
    $ shuffled = shuffle([1, 2, 3])
    $ up = range(5)           # [0..4]
    $ desc = reversed(sorted([3, 1, 2]))
```

Full list: `random randint srand len str int float bool abs min max
round choice sample shuffle range sorted reversed keys values items has
get` plus string helpers `upper lower strip lstrip rstrip split replace
startswith endswith count find index join`.

All builtins also work as read-only methods — values are immutable:

```vn
label start:
    $ u = name.upper()
    $ hits = items.count(x)
    $ csv = ", ".join(bag)
    $ words = "a b c".split(" ")
```

### User functions

`def name(params):` defines a function. In expressions it runs a pure
interpreter — only side-effect-free statements (`$`, `if`, `while`,
`for`, `break`, `continue`, `return`, nested `def`) are allowed.
`return` yields the value; a bare `return` yields `None`.

```vn
def double_it(x):
    return x * 2

def describe(hero):
    $ msg = hero.name + " is level " + str(hero.level)
    return msg

label start:
    e "double_it(21) = [double_it(21)]"
    e "[describe(hero)]"
```

`call name` runs a function body as a **call frame** — blocking
statements (dialogue, menus…) are allowed, and argument bindings persist
after the call:

```vn
def thought:
    n "A quiet thought crosses my mind..."
    return

label start:
    call thought
```

---

## Story flow

```vn
label start:
    jump market

label market(a, b):
    # `call` returns here; `jump` does not.
    call errand("bread", 2)
    end

label errand(what, n):
    n "Bought [n]x [what]."
    return
```

- `jump label` — go to a label, never return.
- `call label` / `call label(arg, ...)` — push a call frame; `return` pops it.
- `label name(a, b):` — declares parameters, bound by `call`.
- `call name` / `call name(args)` — run a `def` body as a call frame.
- `jump [expr]` / `call [expr]` — dynamic target evaluated at runtime;
  a bare identifier stays a literal label for backward compatibility.
- `end` — end the game (returns to the title/exit path).

### Conditionals and loops

```vn
label start:
    if score >= 100:
        e "Perfect!"
    elif score >= 50:
        e "Good."
    else:
        e "Try again."

    $ countdown = 3
    while countdown > 0:
        n "[countdown]..."
        $ countdown -= 1

    for color in ["red", "blue"]:
        n "Color: [color]"

    $ total = 0
    for x in range(10):
        if x == 3:
            continue
        if x > 6:
            break
        $ total += x
```

`break` and `continue` work in `while` and `for`.

### Variables and assignment

```vn
label start:
    $ score = 0               # new variable
    set score = 1             # `set` is an alias for `$`
    $ score += 5              # += -= *= /= %=
    $ d["k"] = "v"            # dict/list targets are allowed
    $ d.k += 1
    $ bag[-1] = "gem"
```

`default name = expr` at top level assigns only if the variable is not
already set (used for save-restore-safe defaults). `define name = expr`
defines a constant.

---

## Dialogue and text

```vn
define e = Character("Eileen", color = "#ff4040")
define nv = Character(None, kind = "nvl")

label start:
    e "Hello!"                        # speaker id + text
    "Narration line, no speaker."
    e "Line with voice." voice "e_hi.ogg"   # per-line voice file
```

`Character(name, color=..., kind=..., voice=..., image=...)`:
`kind="nvl"` renders the line full-window (NVL mode); `voice`/`image` are
per-line defaults.

### Interpolation and rich text

`[expr]` inside dialogue/menu text evaluates full expressions (nested
brackets supported); unresolvable content stays literal. `{b}`,
`{i}`, `{color=#rrggbb}` … `{/color}` mark up the text.

```vn
label start:
    $ player_name = "Ada"
    e "Hi [player_name], score is [score * 2]."
    n "{b}Bold{/b} and {i}italic{/i} and {color=#ff4040}red{/color}."
```

### Custom fonts

Place `.ttf` / `.otf` files under the project's
`fonts/` directory (configurable via `font_dir` in `project.toml`).
All files are loaded at startup and shipped inside `.vnpkg` packages.
Select the default font for all dialogue and UI with a top-level
declaration, and override it per screen element with `font=`:

```vn
font default "MyDisplayFont"

screen hud:
    text "Score: [score]" x=0.02 y=0.015 size=18 font="MyMonoFont"
    button "OK" x=0.8 y=0.5 font="MyDisplayFont":
        return
```

When the named family is not found, the text falls back to the system
sans-serif font.

### Text input and confirmations

```vn
label start:
    input player_name "What's your name?"
    confirm sure "Really, [player_name]?"
    if not sure:
        $ player_name = "Stranger"
    e "Nice to meet you, [player_name]!"
```

Both block until the player answers and bind the given variable
(`input` → text, `confirm` → bool).

### NVL pages and the dialogue window

```vn
label start:
    nv "A full-window line."
    nv "Another one on the same page."
    nvl clear        # start a new NVL page
    window hide      # drop the dialogue box
    scene bg night
    window show      # bring it back
    window auto      # engine decides per line
```

---

## Scenes, sprites and images

```vn
label start:
    scene bg school                    # swap background, hide FG sprites
    scene bg library with blinds 1.0
    scene bg roof onlayer far          # only touches layer `far`
    show eileen happy                  # sprite <char> <image>, FG layer
    show eileen happy at center
    show eileen happy at left, bob with dissolve
    show duo at center layer=mid       # explicit render layer
    hide eileen with dissolve 0.5
    hide duo
    hide all                           # hide every sprite
```

`show <char> <image> at <anchor|transform>, ...` — known anchors are
`left centerleft center centerright right offscreenleft
offscreenright`; other names must be `transform` definitions. `at` is
optional. `layer=<layer>` sets the render layer.

`scene <img> onlayer <layer>` targets `Scene.layer_bgs[<layer>]` and only
hides that layer's sprites.

### Composite images

```vn
image duo:
    "eileen happy" offset=(-0.18, 0.0) zoom=0.9
    "ren smile" offset=(0.18, 0.0) zoom=0.9 alpha=0.95
```

`image <name>:` defines a sprite made of asset layers:
`<asset> [when <expr>] [alpha=<expr>] [offset=(<x>,<y>)] [zoom=<expr>]`.
Used like a normal image: `show duo at center`.

### Inline sprite transforms

```vn
label start:
    transform eileen scale=1.1
    e "She steps closer."
    transform eileen rotation=0.1
    transform eileen rotation=0.0 scale=1.0
```

`transform <char> <prop>=<expr> ...` sets a sprite's props
(`scale`, `rotation`, `offset`, `alpha`, …) instantly.

### Dynamic asset names

`scene`, `show`, `play`, `voice` and `music layer` accept `[expr]` or
expressions evaluated at runtime:

```vn
label start:
    $ outfit = "winter"
    show eileen [outfit]          # shows eileen winter
    scene [location]              # variable background
    play music ["bgm_" + mood]    # computed track
```

---

## Transitions

```vn
label start:
    scene bg school with dissolve 1.5
    scene bg night with fade
    show eileen happy at center with slideright 0.6
    hide eileen with fadewhite 0.8
    with fadewhite                   # standalone transition statement
    pause                            # wait for player input
    pause 1.5                        # timed pause
```

`with <name> [duration]` — seconds; omit for the default duration.
Transition names:

| Family | Names |
|---|---|
| Instant/fade | `cut`, `fade`, `fadewhite`, `dissolve` |
| Slide | `slideleft`, `slideright`, `slideup`, `slidedown` |
| Zoom/push | `zoom`, `pushleft`, `pushright`, `pushup`, `pushdown` |
| Masked | `blinds`, `blindsh`, `blindsv`, `irisin`, `irisout`, `pixelate`, `squares` |

`say`/narration lines and `menu` accept a trailing `with <trans>` — the
transition plays first, then the line/menu appears.

`shake intensity=<e> duration=<e>` shakes the scene.

---

## Menus

```vn
label start:
    menu:
        "Go left":
            $ went_left = True
            jump left_path
        "Go right" if score > 50:    # conditional choice
            jump right_path

label hub:
    menu "Where to?" with dissolve set choice_idx:
        "Library":
            jump library
        "Home":
            jump home
```

`menu [prompt] [with <trans>] [set <var>]:` — each `"text":` entry is a
choice; `if <expr>` hides a choice; `set <var>` stores the chosen
0-based index.

---

## Audio

```vn
label start:
    play music "bgm_calm.ogg" fade 2.0
    stop music fade 1.0
    play sfx "click.wav" volume 0.8
    voice "e_line_01.ogg"
    stop voice
    stop all fade=2.0                 # channels + music + voice
```

### Named channels

```vn
label start:
    play channel amb "sfx_wind.ogg" volume=0.3 fade=2.0 loop
    queue channel seq "sfx_footstep.ogg" volume=0.6
    queue channel seq "sfx_door.ogg" volume=0.5
    volume channel amb 0.65 fade=1.0
    filter channel amb lowpass cutoff=600 mix=0.4
    filter channel amb reverb feedback=0.3 damping=0.5 width=0.8 mix=0.3
    filter channel amb off
    stop channel seq fade=0.5
```

`play channel <n> <track>` replaces the channel and clears its queue;
`queue` appends (starts immediately when idle). Filters: `lowpass`,
`highpass`, `bandpass`, `notch` (`cutoff=`, `resonance=`, `mix=`) and
`reverb` (`feedback=`, `damping=`, `width=`, `mix=`). The channel name is
a literal ident/string or `[expr]`.

### Adaptive music layers

```vn
label start:
    play music "bgm_rooftop.ogg" fade 2.0
    music layer "wind" "sfx_wind.ogg" volume=0.25 fade=2.0
    music layer "wind" volume=0.6 fade=1.0   # retune
    music layer "wind" off                    # remove stem
```

Stems seek to the base BGM position. `play`/`stop music` clears all
stems. Layers survive saves and rollback.

---

## Screens (declarative UI)

```vn
screen hud:
    box x=0.0 y=0.0 w=1.0 h=0.06 color="#000000"
    text "Score: [score]" x=0.02 y=0.015 size=18 color="#ffcc00"
    image "icon_star" x=0.9 y=0.01 w=0.06 h=0.04 alpha=0.9
    button "Reset" x=0.88 y=0.008 w=0.1 h=0.045:
        set score = 0

screen greet:
    box x=0.3 y=0.3 w=0.4 h=0.4 color="#101030"
    text "How do you greet her?" x=0.35 y=0.36 size=26 color="#ffffff"
    if show_extra:
        text "VIP option" x=0.35 y=0.44 size=18 color="#ffcc00"
    for opt in options:
        button "[opt]" x=0.35 y=0.5 w=0.3 h=0.06:
            return opt
    button "Wave hello" x=0.35 y=0.46 w=0.3 h=0.06:
        return "wave"
    button "Leave" x=0.35 y=0.62 w=0.3 h=0.06:
        jump start
```

`screen <name>:` defines UI in normalized 0..1 coordinates. Elements:
`text <e>`, `box`, `image <e>`, `button <e>:` — each takes `x= y= w= h=`
plus styling (`size=`, `color=`, `alpha=`). `if <e>:` / `for <v> in
<e>:` expand at resolution time.

Button actions (indented under the button): `jump <label>`,
`set <target> = <e>`, `return [<e>]`, `show screen <name>`,
`hide screen <name>` — several actions per button are allowed; `set` and
`return` expressions evaluate **at click time**.

```vn
label start:
    show screen hud              # non-blocking overlay
    call screen greet set g      # blocks until return/jump; g = return value
    hide screen hud
```

---

## ATL animations

```vn
transform bob:
    ease 0.8 yoffset -0.015
    ease 0.8 yoffset 0.0
    repeat

transform fadein:
    on show:
        alpha 0.0
        ease 0.4 alpha 1.0
    on hide:
        ease 0.4 alpha 0.0

transform slide_in:
    linear 0.5 xpos 0.5 alpha 1.0
    pause 0.5
    choice 3:
        ease 0.3 rotate 0.05
    choice 1:
        ease 0.3 rotate -0.05
    parallel:
        ease 1.0 zoom 1.1
```

`transform <name>:` defines an animation block attached with
`show ... at <name>`:

- `[warper] <dur> <prop> <expr> ...` — tween: warp functions `linear
  ease easein easeout bounce elastic` (unknown → `linear`).
- Bare `<prop> <expr>` lines — set instantly.
- `pause <s>` — wait; `time <t>` — sync to track elapsed.
- `repeat [n]` — repeat the preceding steps (`repeat` alone = forever).
- `parallel:` — split into a second track.
- `choice [weight]:` blocks — pick one branch via the serialized PRNG.
- `on show:` / `on replace:` / `on hide:` — event tracks fired on
  attach/replace/hide.

Properties: `xpos ypos xalign yalign zoom xzoom yzoom alpha rotate
xoffset yoffset xanchor yanchor z` (`rotate` is radians).

---

## Lighting and camera (2.5D)

```vn
label start:
    ambient 0.45 0.4 0.55
    light "sun" x=0.78 y=0.18 radius=0.65 intensity=1.1 color="#ff9040"
    light "city" x=0.2 y=0.95 radius=0.5 intensity=0.4 color="#4466ff"
    camera x=0.0 y=0.02 zoom=1.06 over=2.0
    unlight "sun"
    ambient 1.0 1.0 1.0
    camera x=0.0 y=0.0 zoom=1.0
```

`camera ... over=<sec>` animates with a smoothstep on the logical clock.

---

## Achievements and persistent data

```vn
achievement true_end "Reach the true ending"
achievement collector "Find every item"

label start:
    grant true_end                    # name must be declared
    grant [dynamic_name]              # dynamic form
    $ persistent.true_end = True      # survives saves & rollback
    $ persistent.endings = persistent.get("endings", 0) + 1
    if "collector" in achievements:
        n "All achievements unlocked!"
```

`persistent` is a dict injected into every run — it lives **outside**
save files and is never reverted by rollback. `achievements` is the list
of granted names. The host flushes `persistent.json` next to the saves.

---

## Plugins

```vn
label start:
    plugin journal "chapter", "gate"
```

`plugin <name> [<expr>, ...]` calls a sandboxed WASM plugin
(`plugins/<name>.wasm` + `<name>.toml` manifest in the project). The
arguments go to the plugin's `vn_on_command` export as a plain-JSON
array.

---

## Statement summary

| Statement | Form |
|---|---|
| `define` | `define id = <expr>` (constant / `Character(...)`) |
| `default` | `default name = <expr>` (only if unset) |
| `def` | `def name(params): <body>` |
| `label` | `label name[(params)]: <body>` |
| `jump` / `call` | `jump label` / `call label(args)` / `call func(args)` |
| `return` / `end` | `return [expr]` / `end` |
| `if`/`elif`/`else` | `if <e>: ... elif <e>: ... else:` |
| `while` / `for` | `while <e>:` / `for v in <e>:` |
| `break` / `continue` | loop control |
| `$` / `set` | `$ target [op]= <expr>` |
| dialogue | `speaker "text" [voice <e>]` / `"text"` |
| `scene` | `scene <img> [with <t> [dur]] [onlayer <layer>]` |
| `show` | `show <char> <img> [at <a>,...] [layer=<l>] [with <t>]` |
| `hide` | `hide <char> [with <t>]` / `hide all` |
| `with` | `with <trans> [dur]` |
| `pause` | `pause [seconds]` |
| `menu` | `menu [prompt] [with <t>] [set <var>]: <choices>` |
| `play`/`stop` | `play music|sfx <track> [volume=<e>] [fade=<e>] [loop]` |
| `voice` | `voice <track>` / `stop voice` |
| channels | `play|queue|stop|volume|filter channel <n> ...` |
| `music layer` | `music layer "<n>" [<track>] [volume=<e>] [fade=<e>]` / `off` |
| `screen` defs | `screen <name>:` — `text`/`box`/`image`/`button`/`if`/`for` |
| screens | `show|hide screen <name>` / `call screen <name> [set <var>]` |
| `input`/`confirm` | `input <var> ["prompt"]` / `confirm <var> ["msg"]` |
| `nvl` / `window` | `nvl clear` / `window show|hide|auto` |
| `transform` def | `transform <name>:` ATL block |
| `transform` stmt | `transform <char> <prop>=<e> ...` |
| `image` | `image <name>:` composite layers |
| `light`/`unlight`/`ambient`/`camera` | 2.5D scene lighting |
| `achievement`/`grant` | `achievement <name> ["desc"]` / `grant <name>` |
| `plugin` | `plugin <name> [<expr>, ...]` |
| `shake` | `shake intensity=<e> duration=<e>` |
