#!/usr/bin/env python3
"""AxiomVault Computer Use Test Harness.

Drives the AxiomVault desktop app via the Claude CLI. Claude uses its
built-in Bash tool (for xdotool/scrot) and Read tool (to view screenshots)
to interact with the application UI.

Uses your Claude CLI subscription — no ANTHROPIC_API_KEY needed.

Requirements:
  - claude CLI installed and authenticated
  - xdotool (X11 input simulation)
  - scrot (X11 screenshots)

Usage:
  python harness.py --scenario smoke_test
  python harness.py --scenario e2e_full_lifecycle
  python harness.py "Click the + button and create a vault called demo"
  python harness.py --list-scenarios
"""

import argparse
import os
import subprocess
import sys

from scenarios import SCENARIOS

SCREENSHOT_PATH = "/tmp/axiomvault_screenshot.png"
DISPLAY_WIDTH = 1024
DISPLAY_HEIGHT = 768
MAX_TURNS = int(os.environ.get("AXIOM_CU_MAX_TURNS", "50"))

TOOL_INSTRUCTIONS = f"""\
You are a QA tester for the AxiomVault desktop application.
The app is already running in an X11 display ({DISPLAY_WIDTH}x{DISPLAY_HEIGHT}).

## How to interact with the app

**Take a screenshot** — run with Bash, then view with Read:
  scrot -o {SCREENSHOT_PATH}
Then read {SCREENSHOT_PATH} to see the current screen.

**Mouse actions** (via Bash):
  xdotool mousemove X Y click 1              # left click at (X, Y)
  xdotool mousemove X Y click 3              # right click
  xdotool mousemove X Y click --repeat 2 1   # double click

**Type text** (via Bash):
  xdotool type --delay 30 --clearmodifiers "text here"

**Press keys** (via Bash):
  xdotool key --clearmodifiers Return
  xdotool key --clearmodifiers ctrl+a
  xdotool key --clearmodifiers Tab

**Important rules:**
- ALWAYS start by taking a screenshot to see the current state.
- After every action, take a screenshot to verify the result.
- Add `sleep 0.3` after xdotool commands before screenshotting.
- For each test step, report PASS or FAIL with a brief explanation.
- If a step fails, continue with the remaining steps.
- Report all results at the end.
"""


def run_scenario(task: str, model: str | None = None,
                 max_turns: int = MAX_TURNS) -> bool:
    """Run a test scenario using the claude CLI."""
    full_prompt = TOOL_INSTRUCTIONS + "\n## Your Task\n\n" + task

    cmd = [
        "claude",
        "-p", full_prompt,
        "--dangerously-skip-permissions",
        "--max-turns", str(max_turns),
        "--output-format", "text",
    ]

    if model:
        cmd.extend(["--model", model])

    # Remove CLAUDECODE env var to allow running from within a Claude Code terminal
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"}
    result = subprocess.run(cmd, env=env)
    return result.returncode == 0


def main():
    parser = argparse.ArgumentParser(
        description="AxiomVault Computer Use Test Harness (Claude CLI)"
    )
    parser.add_argument("task", nargs="?", help="Natural language task")
    parser.add_argument("--scenario", "-s", help="Run a named scenario")
    parser.add_argument("--list-scenarios", "-l", action="store_true",
                        help="List available test scenarios")
    parser.add_argument("--model", "-m", default=None, help="Model override")
    parser.add_argument("--max-turns", type=int, default=MAX_TURNS,
                        help=f"Max conversation turns (default: {MAX_TURNS})")
    args = parser.parse_args()

    if args.list_scenarios:
        print("Available scenarios:\n")
        for name, info in SCENARIOS.items():
            print(f"  {name:20s}  {info['description']}")
        return

    if args.scenario:
        if args.scenario not in SCENARIOS:
            print(f"Unknown scenario: {args.scenario}")
            print(f"Available: {', '.join(SCENARIOS.keys())}")
            sys.exit(1)
        scenario = SCENARIOS[args.scenario]
        task = scenario["prompt"]
        print(f"Running scenario: {args.scenario}")
        print(f"  {scenario['description']}\n")
    elif args.task:
        task = args.task
    else:
        parser.print_help()
        sys.exit(1)

    success = run_scenario(task, model=args.model, max_turns=args.max_turns)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
