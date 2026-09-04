import pytest

from integration.helpers import make_world


@pytest.fixture
def world(tmp_path):
    yield from make_world(tmp_path)
