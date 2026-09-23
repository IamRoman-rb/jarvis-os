"""Tests de humo del esqueleto (fase 0)."""

import importlib

import pytest
from typer.testing import CliRunner

import jarvis
from jarvis.cli import app

SUBPACKAGES = ["core", "agent", "tools", "policy", "voice", "service", "remote"]


@pytest.mark.parametrize("name", SUBPACKAGES)
def test_subpackages_importables(name: str) -> None:
    importlib.import_module(f"jarvis.{name}")


def test_cli_version() -> None:
    result = CliRunner().invoke(app, ["version"])
    assert result.exit_code == 0
    assert jarvis.__version__ in result.output
