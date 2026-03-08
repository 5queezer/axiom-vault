#!/usr/bin/env python3
"""AxiomVault Computer Use Test Harness.

Drives the AxiomVault desktop app via Claude's computer use API.
Claude sees screenshots and issues mouse/keyboard actions to test the UI.

Requirements:
  - xdotool (X11 input simulation)
  - scrot (X11 screenshots)
  - anthropic Python SDK
  - ANTHROPIC_API_KEY environment variable

Usage:
  python harness.py "Create a vault called test-vault with password secret123"
  python harness.py --scenario smoke_test
  python harness.py --list-scenarios
"""

import argparse
import base64
import os
import subprocess
import sys
import time

import anthropic

from scenarios import SCENARIOS

# Match tauri.conf.json window dimensions
DISPLAY_WIDTH = 1024
DISPLAY_HEIGHT = 768
MODEL = os.environ.get("AXIOM_CU_MODEL", "claude-sonnet-4-20250514")
MAX_STEPS = int(os.environ.get("AXIOM_CU_MAX_STEPS", "50"))
SCREENSHOT_PATH = "/tmp/axiomvault_screenshot.png"


class ComputerController:
    """Execute screenshot and input actions via X11 tools (xdotool, scrot)."""

    def screenshot(self) -> str:
        """Take a screenshot and return base64-encoded PNG."""
        subprocess.run(
            ["scrot", "-o", SCREENSHOT_PATH],
            check=True,
            capture_output=True,
        )
        with open(SCREENSHOT_PATH, "rb") as f:
            return base64.standard_b64encode(f.read()).decode()

    def execute(self, action: dict) -> str | None:
        """Execute a computer use action. Returns result text, or None for screenshots."""
        name = action["action"]

        if name == "screenshot":
            return None

        coord = action.get("coordinate")

        if name == "left_click":
            self._click(coord, "1")
        elif name == "right_click":
            self._click(coord, "3")
        elif name == "double_click":
            self._xdotool("mousemove", str(coord[0]), str(coord[1]),
                          "click", "--repeat", "2", "1")
        elif name == "middle_click":
            self._click(coord, "2")
        elif name == "mouse_move":
            self._xdotool("mousemove", str(coord[0]), str(coord[1]))
        elif name == "type":
            self._xdotool("type", "--delay", "30", "--clearmodifiers", action["text"])
        elif name == "key":
            self._xdotool("key", "--clearmodifiers", self._map_keys(action["text"]))
        elif name == "scroll":
            self._scroll(coord, action.get("scroll_direction", "down"),
                         action.get("scroll_amount", 3))
        elif name == "cursor_position":
            return self._get_cursor_position()
        else:
            return f"Unknown action: {name}"

        time.sleep(0.3)  # let the UI settle
        return f"Action '{name}' executed"

    def _click(self, coord: list[int], button: str):
        self._xdotool("mousemove", str(coord[0]), str(coord[1]), "click", button)

    def _scroll(self, coord: list[int], direction: str, amount: int):
        self._xdotool("mousemove", str(coord[0]), str(coord[1]))
        # xdotool: button 4 = scroll up, 5 = scroll down
        button = "4" if direction in ("up", "left") else "5"
        for _ in range(amount):
            self._xdotool("click", button)

    def _get_cursor_position(self) -> str:
        result = subprocess.run(
            ["xdotool", "getmouselocation"],
            capture_output=True, text=True, check=True,
        )
        parts = result.stdout.strip().split()
        x = parts[0].split(":")[1]
        y = parts[1].split(":")[1]
        return f"x={x},y={y}"

    @staticmethod
    def _map_keys(keys: str) -> str:
        """Map Claude key names to xdotool key names."""
        replacements = {
            "Enter": "Return",
            "Backspace": "BackSpace",
            "Space": "space",
            "ArrowUp": "Up",
            "ArrowDown": "Down",
            "ArrowLeft": "Left",
            "ArrowRight": "Right",
            "Control_L": "ctrl",
            "Alt_L": "alt",
            "Super_L": "super",
        }
        for old, new in replacements.items():
            keys = keys.replace(old, new)
        return keys

    @staticmethod
    def _xdotool(*args: str):
        subprocess.run(["xdotool", *args], check=True, capture_output=True)


def make_screenshot_content(b64: str) -> dict:
    return {
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/png",
            "data": b64,
        },
    }


def run_agent_loop(
    task: str,
    controller: ComputerController,
    model: str = MODEL,
    max_steps: int = MAX_STEPS,
) -> bool:
    """Run the computer use agent loop. Returns True if Claude completes the task."""
    client = anthropic.Anthropic()

    screenshot_b64 = controller.screenshot()

    messages = [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": task},
                make_screenshot_content(screenshot_b64),
            ],
        }
    ]

    tools = [
        {
            "type": "computer_20250124",
            "name": "computer",
            "display_width_px": DISPLAY_WIDTH,
            "display_height_px": DISPLAY_HEIGHT,
        }
    ]

    for step in range(max_steps):
        print(f"\n--- Step {step + 1}/{max_steps} ---")

        response = client.beta.messages.create(
            model=model,
            max_tokens=4096,
            tools=tools,
            messages=messages,
            betas=["computer-use-2025-01-24"],
        )

        assistant_content = response.content
        messages.append({"role": "assistant", "content": assistant_content})

        # Check if Claude is done
        if response.stop_reason == "end_turn":
            for block in assistant_content:
                if hasattr(block, "text"):
                    print(f"Claude: {block.text}")
            print("\nTask completed.")
            return True

        # Process tool use blocks
        tool_results = []
        for block in assistant_content:
            if hasattr(block, "text") and block.text:
                print(f"Claude: {block.text}")
            elif block.type == "tool_use":
                action = block.input
                action_name = action["action"]
                detail = action.get("coordinate", action.get("text", ""))
                print(f"  -> {action_name} {detail}")

                if action_name == "screenshot":
                    screenshot_b64 = controller.screenshot()
                    tool_results.append({
                        "type": "tool_result",
                        "tool_use_id": block.id,
                        "content": [make_screenshot_content(screenshot_b64)],
                    })
                else:
                    result_text = controller.execute(action)
                    screenshot_b64 = controller.screenshot()
                    tool_results.append({
                        "type": "tool_result",
                        "tool_use_id": block.id,
                        "content": [
                            {"type": "text", "text": result_text or "OK"},
                            make_screenshot_content(screenshot_b64),
                        ],
                    })

        messages.append({"role": "user", "content": tool_results})

    print("\nMax steps reached without completion.")
    return False


def main():
    parser = argparse.ArgumentParser(description="AxiomVault Computer Use Test Harness")
    parser.add_argument("task", nargs="?", help="Natural language task for Claude to perform")
    parser.add_argument("--scenario", "-s", help="Run a named scenario from scenarios.py")
    parser.add_argument("--list-scenarios", "-l", action="store_true",
                        help="List available test scenarios")
    parser.add_argument("--model", "-m", default=MODEL, help=f"Model to use (default: {MODEL})")
    parser.add_argument("--max-steps", type=int, default=MAX_STEPS,
                        help=f"Max agent loop iterations (default: {MAX_STEPS})")
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

    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("Error: ANTHROPIC_API_KEY environment variable is required.")
        sys.exit(1)

    controller = ComputerController()
    success = run_agent_loop(task, controller, model=args.model, max_steps=args.max_steps)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
