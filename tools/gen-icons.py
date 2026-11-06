#!/usr/bin/env python3
"""Generate the indicator's SVG icon set into data/icons/hicolor/scalable.

Design: a shield (VPN = protection) carrying a keyhole-shaped tunnel, in the
orange / navy palette OpenVPN users recognise, plus small status badges in
the lower-right corner. Run from the repository root:

    python3 tools/gen-icons.py
"""

from pathlib import Path

ORANGE = "#ED7F22"
ORANGE_DARK = "#C9631A"
NAVY = "#1A3967"
GREY = "#B9C0D4"
GREY_DARK = "#404968"
RED = "#C8102E"
WHITE = "#FFFFFF"

# Shield outline, 256x256 canvas.
SHIELD = (
    "M128 12 L222 46 C224 47 225 49 225 51 V118 "
    "C225 178 185 222 128 246 C71 222 31 178 31 118 V51 "
    "C31 49 32 47 34 46 Z"
)
# Inner rim, slightly smaller, for a subtle bevel.
RIM = (
    "M128 32 L205 60 V118 C205 167 173 204 128 225 "
    "C83 204 51 167 51 118 V60 Z"
)
# Keyhole / tunnel: round head and a flared stem.
KEYHOLE = (
    "M128 70 C150 70 167 87 167 109 C167 124 159 137 146 144 "
    "L156 192 H100 L110 144 C97 137 89 124 89 109 C89 87 106 70 128 70 Z"
)


def shield(body: str, rim: str, hole: str) -> str:
    return (
        f'  <path fill="{body}" d="{SHIELD}"/>\n'
        f'  <path fill="none" stroke="{rim}" stroke-width="6" stroke-linejoin="round" d="{RIM}"/>\n'
        f'  <path fill="{hole}" d="{KEYHOLE}"/>\n'
    )


ACTIVE = shield(ORANGE, ORANGE_DARK, NAVY)
IDLE = shield(GREY, "#A2AAC2", GREY_DARK)

# Badges live in the empty lower-right corner (centre ~ 206,206).
BADGE_ERROR = (
    f'  <path fill="{RED}" stroke="{WHITE}" stroke-width="8" stroke-linejoin="round" '
    'd="M204 142 L248 226 H160 Z"/>\n'
    f'  <rect x="198" y="168" width="12" height="32" rx="6" fill="{WHITE}"/>\n'
    f'  <circle cx="204" cy="212" r="7" fill="{WHITE}"/>\n'
)
BADGE_LOADING = "".join(
    f'  <circle cx="{x}" cy="226" r="22" fill="{ORANGE}" stroke="{WHITE}" stroke-width="6"/>\n'
    for x in (118, 174, 230)
)
BADGE_PAUSED = (
    f'  <circle cx="204" cy="204" r="44" fill="{NAVY}" stroke="{WHITE}" stroke-width="8"/>\n'
    f'  <rect x="184" y="182" width="14" height="44" rx="4" fill="{WHITE}"/>\n'
    f'  <rect x="210" y="182" width="14" height="44" rx="4" fill="{WHITE}"/>\n'
)
ICONS = {
    "apps/openvpn3-indicator-rs": ACTIVE,
    "status/openvpn3-indicator-rs-active": ACTIVE,
    "status/openvpn3-indicator-rs-active-error": ACTIVE + BADGE_ERROR,
    "status/openvpn3-indicator-rs-idle": IDLE,
    "status/openvpn3-indicator-rs-idle-error": IDLE + BADGE_ERROR,
    "status/openvpn3-indicator-rs-loading": IDLE + BADGE_LOADING,
    "status/openvpn3-indicator-rs-paused": IDLE + BADGE_PAUSED,
}


def main() -> None:
    root = Path(__file__).resolve().parent.parent / "data/icons/hicolor/scalable"
    for name, body in ICONS.items():
        path = root / f"{name}.svg"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">\n'
            f"{body}</svg>\n"
        )
        print(path.relative_to(root.parents[2]))


if __name__ == "__main__":
    main()
