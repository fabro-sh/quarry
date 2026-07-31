"""Deliberately vulnerable fixture used only to verify the Fabro workflow."""

import os


def run_report() -> None:
    command = input("Report command: ")
    os.system(command)


if __name__ == "__main__":
    run_report()
