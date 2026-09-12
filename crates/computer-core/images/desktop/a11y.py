#!/usr/bin/env python3
#
# The desktop's accessibility tree, as JSON on stdout.
#
# usage: computer-a11y tree   [--app NAME] [--depth N]
#        computer-a11y find   <query> [--role R] [--limit N] [--exact] [--app NAME]
#        computer-a11y focus  <query> [--role R] [--exact] [--app NAME]
#        computer-a11y invoke <query> [--role R] [--exact] [--app NAME] [--action NAME]
#        computer-a11y set    <query> <value> [--role R] [--exact] [--app NAME]
#
# Every verb takes a query rather than an id, because an AT-SPI path dies when
# the widget behind it is rebuilt: a caller holding one from a previous call is
# holding a handle to whatever took that place. An id is reported so a caller
# can tell two matches apart, and is good until the tree next changes.
import json
import os
import sys

USAGE = """usage: computer-a11y tree   [--app NAME] [--depth N]
       computer-a11y find   <query> [--role R] [--limit N] [--exact] [--app NAME]
       computer-a11y focus  <query> [--role R] [--exact] [--app NAME]
       computer-a11y invoke <query> [--role R] [--exact] [--app NAME] [--action NAME]
       computer-a11y set    <query> <value> [--role R] [--exact] [--app NAME]"""

# Checked before pyatspi is imported, which connects on import: against a box
# that has the bus address from the image and no bus behind it, that connection
# reports an empty desktop rather than an error, and an empty tree reads as an
# application that publishes nothing.
if not os.path.exists("/usr/libexec/at-spi-bus-launcher"):
    sys.exit("this box was built without Feature::Accessibility, so it has no tree")

try:
    import pyatspi
except ImportError:
    sys.exit("this box was built without Feature::Accessibility, so it has no tree")

# How deep a tree read goes when the caller names no number. A full walk of a
# real application is thousands of nodes and one D-Bus round trip each, so the
# default answers the window rather than the whole program.
DEPTH_DEFAULT = 4

# A search reads the whole application rather than one window's worth: a caller
# looking for a button by name does not know how deep the widget sits, and
# VISIT_CEILING is what bounds the read instead.
DEPTH_ANY = 64

# How many matches a find carries by default, and the ceiling on how much of a
# tree one search will visit. The ceiling is what stops a search of a text
# editor's document from taking a minute.
FOUND_DEFAULT = 20
VISIT_CEILING = 4000

# How far from a label the widget it names may sit, in pixels. A form puts the
# two within a few pixels of each other; anything this far apart is two
# unrelated things that happen to be on the same row.
LABEL_REACH = 220

# Roles that do something when pressed or typed into. A query usually names one
# of these, so they rank above a container that merely encloses the words.
ACTIONABLE = {
    "push button",
    "toggle button",
    "radio button",
    "check box",
    "menu item",
    "check menu item",
    "radio menu item",
    "link",
    "entry",
    "text",
    "password text",
    "combo box",
    "list item",
    "table cell",
    "slider",
    "spin button",
    "tab",
}


def labels_of(node):
    """The words of every node that labels this one.

    A GTK entry's own name is empty and its words live in a sibling label, so a
    search that reads names alone finds every button and no field.
    """
    words = []
    try:
        for relation in node.getRelationSet():
            if relation.getRelationType() != pyatspi.RELATION_LABELLED_BY:
                continue
            for index in range(relation.getNTargets()):
                target = relation.getTarget(index)
                if target is not None and target.name:
                    words.append(target.name)
    except Exception:
        pass

    return words


def box_of(extents):
    return (extents.x, extents.y, extents.x + extents.width, extents.y + extents.height)


def names_widget(label, widget):
    """Whether a label sits where it would be naming this widget.

    Geometry rather than the `LABELLED_BY` relation, because most applications
    publish no relation at all — zenity, the reference GTK form, publishes an
    empty relation set for every node, and its labels come *after* their fields
    in tree order besides. What is left is the rule a person reads by: a label
    names the field to its right, or the one under it.
    """
    left, top, right, bottom = label
    other_left, other_top, other_right, other_bottom = widget

    same_row = other_top < bottom and top < other_bottom
    if same_row and 0 <= other_left - right <= LABEL_REACH:
        return other_left - right

    same_column = other_left < right and left < other_right
    if same_column and 0 <= other_top - bottom <= LABEL_REACH:
        return other_top - bottom

    return None


def states_of(node):
    try:
        return sorted(pyatspi.stateToString(state) for state in node.getState().getStates())
    except Exception:
        return []


def actions_of(node):
    try:
        action = node.queryAction()
        return [action.getName(index) for index in range(action.nActions)]
    except NotImplementedError:
        return []
    except Exception:
        return []


def bounds_of(node):
    try:
        extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    except Exception:
        return None

    if extents.width < 1 or extents.height < 1:
        return None

    return extents


def value_of(node):
    try:
        text = node.queryText()
        return text.getText(0, -1)
    except NotImplementedError:
        return None
    except Exception:
        return None


def described(node, node_id, app_name):
    extents = bounds_of(node)
    labels = labels_of(node)

    out = {
        "id": node_id,
        "app": app_name,
        "role": node.getRoleName(),
        "name": node.name or "",
        "actions": actions_of(node),
        "states": states_of(node),
    }

    if labels:
        out["labels"] = labels

    if extents is not None:
        # The middle, in screen coordinates, which is the space a click takes.
        out["at"] = {
            "x": extents.x + extents.width // 2,
            "y": extents.y + extents.height // 2,
        }
        out["width"] = extents.width
        out["height"] = extents.height

    value = value_of(node)
    if value is not None:
        out["value"] = value[:200]

    return out


def applications(app_filter):
    desktop = pyatspi.Registry.getDesktop(0)
    for index in range(desktop.childCount):
        try:
            app = desktop.getChildAtIndex(index)
        except Exception:
            continue
        if app is None:
            continue
        if app_filter and app.name != app_filter:
            continue
        yield index, app


def active_first(apps):
    """The application owning an active window first.

    Two windows can each hold an OK button, and the one the keyboard reaches is
    the one a caller means.
    """

    def is_active(pair):
        _, app = pair
        try:
            for index in range(app.childCount):
                frame = app.getChildAtIndex(index)
                if frame is None:
                    continue
                if frame.getState().contains(pyatspi.STATE_ACTIVE):
                    return 0
        except Exception:
            pass
        return 1

    return sorted(apps, key=is_active)


def walk(app, app_index, depth_limit, visit_limit):
    """Every node under one application, with the id that addresses it."""
    stack = [(app, f"{app_index}", 0)]
    seen = 0

    while stack:
        node, node_id, depth = stack.pop()
        yield node, node_id, depth
        seen += 1

        if seen >= visit_limit or depth >= depth_limit:
            continue

        try:
            children = [
                (node.getChildAtIndex(index), index) for index in range(node.childCount)
            ]
        except Exception:
            continue

        # Reversed, so a pop-driven walk still reports children in tree order.
        for child, index in reversed(children):
            if child is not None:
                stack.append((child, f"{node_id}.{index}", depth + 1))


def resolve(node_id):
    """The node one id addresses, or None where the tree has moved on."""
    parts = node_id.split(".")
    desktop = pyatspi.Registry.getDesktop(0)
    node = desktop

    for part in parts:
        try:
            node = node.getChildAtIndex(int(part))
        except Exception:
            return None
        if node is None:
            return None

    return node


def ranked(query, role, exact, app_filter, limit):
    """Nodes matching a query, best first.

    Exact words beat a substring, a label's words count as the widget's own,
    and something actionable beats the container that encloses it — the same
    ranking the page tools use, for the same reason.
    """
    wanted = query.strip().lower()
    found = {}

    def keep(rank, node, out):
        held = found.get(out["id"])
        if held is None or rank < held[0]:
            found[out["id"]] = (rank, node, out)

    for app_index, app in active_first(list(applications(app_filter))):
        app_name = app.name or ""
        # One walk: every node is a D-Bus round trip, and the label pass below
        # needs the same nodes the match pass reads.
        seen = []
        for node, node_id, _ in walk(app, app_index, DEPTH_ANY, VISIT_CEILING):
            try:
                node_role = node.getRoleName()
            except Exception:
                continue
            seen.append((node, node_id, node_role, bounds_of(node)))

        actionable = [
            (node, node_id, node_role, extents)
            for node, node_id, node_role, extents in seen
            if node_role in ACTIONABLE and extents is not None
        ]

        for node, node_id, node_role, extents in seen:
            words = [node.name or ""] + labels_of(node)
            words = [" ".join(word.split()).lower() for word in words if word.strip()]

            if any(word == wanted for word in words):
                rank = 0
            elif not exact and any(wanted in word for word in words):
                rank = 2
            else:
                continue

            if node_role in ACTIONABLE:
                if not role or node_role == role:
                    keep(rank, node, described(node, node_id, app_name))
                continue

            if not role or node_role == role:
                # The label itself, behind anything it names: a caller asking
                # for "Street" wants the field, and only says so by naming the
                # word beside it.
                keep(rank + 2, node, described(node, node_id, app_name))

            if extents is None:
                continue

            nearest = None
            for other, other_id, other_role, other_extents in actionable:
                if role and other_role != role:
                    continue
                gap = names_widget(box_of(extents), box_of(other_extents))
                if gap is not None and (nearest is None or gap < nearest[0]):
                    nearest = (gap, other, other_id, other_role)

            if nearest is not None:
                _, other, other_id, _ = nearest
                out = described(other, other_id, app_name)
                out["labelled"] = node.name
                keep(rank, other, out)

    return sorted(found.values(), key=lambda triple: triple[0])[:limit]


def matches(query, role, exact, app_filter, limit):
    return [out for _, _, out in ranked(query, role, exact, app_filter, limit)]


def one(query, role, exact, app_filter):
    best = ranked(query, role, exact, app_filter, 1)
    if not best:
        sys.exit(f"nothing in the tree matched {query!r}")

    _, node, out = best[0]
    return node, out


def flag(argv, name, fallback=None):
    if name not in argv:
        return fallback
    at = argv.index(name)
    if at + 1 >= len(argv):
        sys.exit(f"{name} needs a value")
    value = argv[at + 1]
    del argv[at : at + 2]
    return value


def switch(argv, name):
    if name in argv:
        argv.remove(name)
        return True
    return False


def main():
    argv = sys.argv[1:]
    if not argv:
        sys.exit(USAGE)

    verb = argv.pop(0)

    exact = switch(argv, "--exact")
    role = flag(argv, "--role")
    app_filter = flag(argv, "--app")
    action_name = flag(argv, "--action")
    depth = int(flag(argv, "--depth", DEPTH_DEFAULT))
    limit = int(flag(argv, "--limit", FOUND_DEFAULT))

    if verb == "tree":
        out = []
        for app_index, app in active_first(list(applications(app_filter))):
            app_name = app.name or ""
            for node, node_id, _ in walk(app, app_index, depth, VISIT_CEILING):
                out.append(described(node, node_id, app_name))
        print(json.dumps({"nodes": out}))
        return

    if verb == "find":
        if not argv:
            sys.exit("find needs a query")
        print(json.dumps({"nodes": matches(argv[0], role, exact, app_filter, limit)}))
        return

    if verb == "focus":
        if not argv:
            sys.exit("focus needs a query")
        node, out = one(argv[0], role, exact, app_filter)
        try:
            took = node.queryComponent().grabFocus()
        except Exception as error:
            sys.exit(f"{out['role']} {out['name']!r} would not take focus: {error}")
        if not took:
            sys.exit(f"{out['role']} {out['name']!r} refused focus")
        print(json.dumps({"node": out}))
        return

    if verb == "invoke":
        if not argv:
            sys.exit("invoke needs a query")
        node, out = one(argv[0], role, exact, app_filter)
        names = out["actions"]
        if not names:
            sys.exit(
                f"{out['role']} {out['name']!r} publishes no action, so there is "
                f"nothing to invoke. Its bounds are in the answer to `find`: click it."
            )

        # Action 0 unless the caller named one: GTK calls it "click" and Qt
        # calls it "Press", and a caller that must know the toolkit before it
        # can press a button has gained nothing over a coordinate.
        index = 0
        if action_name:
            wanted = action_name.strip().lower()
            index = next(
                (at for at, name in enumerate(names) if name.strip().lower() == wanted),
                None,
            )
            if index is None:
                sys.exit(f"no action named {action_name!r}, of {names}")

        try:
            node.queryAction().doAction(index)
        except Exception as error:
            sys.exit(f"{names[index]} on {out['name']!r} failed: {error}")

        print(json.dumps({"node": out, "action": names[index]}))
        return

    if verb == "set":
        if len(argv) < 2:
            sys.exit("set needs a query and a value")
        node, out = one(argv[0], role, exact, app_filter)
        try:
            editable = node.queryEditableText()
        except NotImplementedError:
            sys.exit(
                f"{out['role']} {out['name']!r} is not editable, so a value "
                f"cannot be written to it"
            )
        if not editable.setTextContents(argv[1]):
            sys.exit(f"{out['role']} {out['name']!r} refused the value")

        out["value"] = value_of(node)
        print(json.dumps({"node": out}))
        return

    sys.exit(f"no such verb: {verb}")


if __name__ == "__main__":
    main()
