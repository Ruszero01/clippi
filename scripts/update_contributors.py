"""Generate the README wall using gh CLI and Python's standard library.

Run: python scripts/update_contributors.py
Requires an authenticated gh CLI (GH_TOKEN in Actions). Manual credits and
exclusions live in .github/contributors.json; README files need no updates.
"""

import base64
from functools import lru_cache
import html
import json
import math
from pathlib import Path
import re
import random
import subprocess
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parent.parent


@lru_cache(maxsize=1)
def outline_font():
    return json.loads((ROOT / ".github" / "contributors-font.json").read_text(encoding="utf-8"))


def outlined_text(text, x, y, size, weight, fill, anchor="start", spacing=0, max_width=None):
    """Draw glyph outlines so neither the browser nor CI chooses a fallback font."""
    font = outline_font()
    face = font["faces"][str(weight)]
    scale = size / font["units_per_em"]
    advances = [face[char]["advance"] for char in text]
    width = sum(advances) * scale + max(0, len(text) - 1) * spacing
    if max_width and width > max_width:
        scale *= max_width / width
        spacing *= max_width / width
        width = max_width
    start = x - {"start": 0, "middle": width / 2, "end": width}[anchor]
    paths, cursor = [], 0
    for char, advance in zip(text, advances):
        paths.append(f'<path d="{face[char]["path"]}" transform="translate({cursor:g} 0)"/>')
        cursor += advance + spacing / scale
    label = html.escape(text, quote=True)
    return (f'<g role="img" aria-label="{label}" fill="{fill}" '
            f'transform="translate({start:g} {y:g}) scale({scale:g} {-scale:g})">'
            + "".join(paths) + "</g>")


def background_dots(width, height, color, dark):
    """Deterministic irregular spacing and density without animation in README."""
    rng = random.Random(122)
    circles = []
    for y in range(12, height, 16):
        for x in range(12, width, 16):
            field = (1 + math.sin(x / 110 + y / 95)) / 2
            if rng.random() > .25 + .45 * field:
                continue
            cx, cy = x + rng.uniform(-3, 3), y + rng.uniform(-3, 3)
            radius = rng.uniform(.4, .85)
            opacity = rng.uniform(.04, .13) if dark else rng.uniform(.09, .24)
            circles.append(f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="{radius:.2f}" opacity="{opacity:.2f}"/>')
    return f'<g id="background-dots" fill="{color}">' + "".join(circles) + '</g>'


def api(endpoint):
    result = subprocess.run(
        ["gh", "api", endpoint], check=True, capture_output=True,
        text=True, encoding="utf-8", timeout=60,
    )
    return json.loads(result.stdout)


def pages(endpoint):
    page = 1
    while True:
        entries = api(f"{endpoint}&per_page=100&page={page}")
        yield from entries
        if len(entries) < 100:
            return
        page += 1


def collect(config):
    repository = config["repository"]
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise ValueError("Invalid repository name")
    repo = api(f"repos/{repository}")
    excluded = {login.lower() for login in config["exclude"]}
    people = {}

    def add(user, contributions=0):
        login = user["login"]
        key = login.lower()
        if user.get("type") != "User" or key in excluded:
            return
        if key not in people:
            people[key] = {"login": login, "contributions": contributions}

    for user in pages(f"repos/{repository}/contributors?anon=false"):
        add(user, user.get("contributions", 0))
    # Merged PR authors remain credited even if their commit email is unlinked.
    branch = urllib.parse.quote(repo["default_branch"], safe="")
    for pr in pages(f"repos/{repository}/pulls?state=closed&base={branch}"):
        if pr["merged_at"] and pr.get("user"):
            add(pr["user"])
    for login in config["include"]:
        if not re.fullmatch(r"[A-Za-z0-9-]+", login):
            raise ValueError(f"Invalid GitHub login: {login}")
        if login.lower() not in people:
            add(api(f"users/{login}"))
    if not people:
        raise ValueError("Refusing to replace the wall with an empty list")
    return sorted(people.values(), key=lambda p: (-p["contributions"], p["login"].lower()))


@lru_cache(maxsize=None)
def avatar(login):
    url = api(f"users/{login}")["avatar_url"]
    url += ("&" if "?" in url else "?") + "s=128"
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "Clippi-contributors"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        mime = response.headers.get_content_type()
        if mime not in {"image/png", "image/jpeg", "image/webp"}:
            raise ValueError(f"Unexpected avatar format for {login}: {mime}")
        content = base64.b64encode(response.read()).decode("ascii")
    return f"data:{mime};base64,{content}"


def render(people, dark=False):
    # A fixed brand header and six-column grid work for any community size.
    palette = (
        dict(bg="#191a1b", surface="#232425", ink="#eaebec", muted="#a0a4a6",
             line="#343738", accent="#7ecba3", stripe="#74bb96", edge="#353e39",
             shadow="#000000", slot="#8ca99b", glow="#30654b")
        if dark else
        dict(bg="#f5f7f8", surface="#ffffff", ink="#202a33", muted="#56636f",
             line="#d7dfe3", accent="#6faf8e", stripe="#639e7e", edge="#cdd6d2",
             shadow="#4c5d69", slot="#8ba99a", glow="#bce5cf")
    )
    bg, surface, ink = (palette[key] for key in ("bg", "surface", "ink"))
    accent, stripe, line = (palette[key] for key in ("accent", "stripe", "line"))
    width, columns, pitch, row_height = 840, 6, 128, 130
    height = 140 + math.ceil(len(people) / columns) * row_height + 14
    names = html.escape(", ".join(person["login"] for person in people))
    # Embed the original artwork: external images inside SVGs are blocked by
    # many README renderers. Do not redraw or substitute the application icon.
    icon = base64.b64encode((ROOT / "assets" / "LOGO_notext.png").read_bytes()).decode("ascii")
    parts = [f'''<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title description">
<title id="title">Clippi contributors</title>
<desc id="description">{names}</desc>
<!-- Generated by scripts/update_contributors.py. Manual credits: .github/contributors.json. -->
<defs>
  <pattern id="stripes" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(135)">
    <rect width="3" height="6" fill="{accent}"/><rect x="3" width="3" height="6" fill="{stripe}"/>
  </pattern>
  <pattern id="hatch" width="7" height="7" patternUnits="userSpaceOnUse" patternTransform="rotate(135)">
    <path d="M0 0v7" stroke="{palette['slot']}" stroke-opacity=".14"/>
  </pattern>
  <radialGradient id="ambient"><stop stop-color="{palette['glow']}" stop-opacity=".2"/><stop offset="1" stop-color="{palette['glow']}" stop-opacity="0"/></radialGradient>
  <filter id="lift" x="-30%" y="-30%" width="170%" height="180%"><feDropShadow dx="0" dy="6" stdDeviation="6" flood-color="{palette['shadow']}" flood-opacity=".13"/></filter>
  <clipPath id="canvas"><rect width="{width}" height="{height}" rx="20"/></clipPath>
  <clipPath id="portrait"><rect x="24" y="14" width="62" height="62" rx="17"/></clipPath>
  <mask id="wordmark-mask">{outlined_text("Clippi", 117, 79, 54, 650, "#ffffff", spacing=-3)}</mask>
</defs>
<g clip-path="url(#canvas)">
  <rect width="{width}" height="{height}" fill="{bg}"/>
  {background_dots(width, height, accent, dark)}
  <ellipse cx="650" cy="100" rx="300" ry="180" fill="url(#ambient)"/>
  <image id="brand-icon" x="30" y="21" width="76" height="76" xlink:href="data:image/png;base64,{icon}"/>
  <rect x="110" y="25" width="180" height="68" fill="url(#stripes)" mask="url(#wordmark-mask)"/>
  {outlined_text("Contributors", 788, 73, 18, 400, palette['muted'], anchor="end")}
  <path d="M38 114H802" stroke="{line}"/>
''']
    for index, person in enumerate(people):
        row, col = divmod(index, columns)
        count = min(columns, len(people) - row * columns)
        x = (width - (count - 1) * pitch - 110) / 2 + col * pitch
        y = 140 + row * row_height
        login = html.escape(person["login"], quote=True)
        image = avatar(person["login"])
        label = login if len(login) <= 13 else login[:11] + "…"
        parts.append(f'''<g class="contributor" transform="translate({x:g} {y})">
  <title>{login}</title>
  <rect x="-4" y="5" width="110" height="108" rx="16" fill="url(#hatch)" stroke="{palette['slot']}" stroke-dasharray="3 4" stroke-opacity=".22"/>
  <g filter="url(#lift)">
    <rect y="3" width="110" height="108" rx="16" fill="{palette['edge']}"/>
    <rect width="110" height="108" rx="16" fill="{surface}" stroke="{line}"/>
    <path d="M16 .7H94" stroke="#ffffff" stroke-opacity="{'.09' if dark else '.8'}"/>
    <rect x="21" y="11" width="68" height="68" rx="20" fill="{bg}" stroke="{palette['edge']}"/>
    <image class="avatar" x="24" y="14" width="62" height="62" clip-path="url(#portrait)" xlink:href="{image}"/>
    {outlined_text(label, 55, 96, 13, 600, ink, anchor="middle", max_width=94)}
  </g>
</g>''')
    parts.append(f'''</g>
<rect x=".5" y=".5" width="{width - 1}" height="{height - 1}" rx="20" fill="none" stroke="{line}"/>
</svg>
''')
    return "\n".join(parts)


def main():
    config = json.loads((ROOT / ".github" / "contributors.json").read_text(encoding="utf-8"))
    people = collect(config)
    # Fetch and render everything before touching the existing image.
    walls = {"contributors.svg": render(people), "contributors-dark.svg": render(people, dark=True)}
    for name, svg in walls.items():
        destination = ROOT / "docs" / "images" / name
        if not destination.exists() or destination.read_text(encoding="utf-8") != svg:
            destination.write_text(svg, encoding="utf-8", newline="\n")
    print(f"Contributors: {', '.join(person['login'] for person in people)}")


if __name__ == "__main__":
    main()
