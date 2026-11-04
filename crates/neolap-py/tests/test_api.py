"""The API, once per backend (`db` is the local file, then the server)."""

import math

import pytest

import neolap


def cube(db):
    return db.cube("sales")


def test_cubes_and_dimensions(db):
    assert db.cubes == ["sales"]
    c = db["SALES"]  # case-insensitive
    assert c.name == "sales"
    assert c.dimensions == ["years", "regions", "products", "measures"]
    assert "sales" in repr(c)


def test_unknown_cube_names_the_available_ones(db):
    with pytest.raises(neolap.NotFoundError, match="available: sales"):
        db.cube("nope")


def test_reading_cells_base_and_aggregate_with_rules(db):
    c = cube(db)
    assert c["Jan", "North", "sedan", "Sales"] == 1000.0
    assert c["Year", "Total", "All", "Sales"] == 165600.0
    assert c["Year", "Total", "All", "Profit"] == 57960.0
    assert c["Year", "Total", "All", "Profit in %"] == pytest.approx(0.35)
    assert c.get(("Year", "Total", "All", "Profit in %"), rules=False) is None  # no stored values


def test_a_member_name_is_case_insensitive_and_an_empty_cell_is_none(db):
    c = cube(db)
    assert c["jan", "north", "SEDAN", "sales"] == 1000.0
    c["Jan", "North", "sedan", "Sales"] = None  # delete
    assert c["Jan", "North", "sedan", "Sales"] is None


def test_wrong_addresses_fail_early_and_clearly(db):
    c = cube(db)
    with pytest.raises(neolap.InputError, match="4 dimensions .*years.*3 members"):
        c["Jan", "North", "sedan"]
    with pytest.raises(neolap.InputError, match="must be a name"):
        c["Jan", "North", "sedan", 5]
    with pytest.raises(neolap.NotFoundError, match="Atlantis"):
        c["Jan", "Atlantis", "sedan", "Sales"]


def test_writing_a_base_cell_and_an_aggregate_splash(db):
    c = cube(db)
    c["Jan", "North", "sedan", "Sales"] = 2000  # was 1000
    assert c["Jan", "North", "sedan", "Sales"] == 2000.0
    assert c["Q1", "Total", "All", "Sales"] == 34840.0  # 33,840 + 1,000
    assert c["Year", "Total", "All", "Sales"] == 166600.0
    changed = c.write(("Q1", "Total", "All", "Sales"), 100000)
    assert changed == 24  # 4 regions x 2 products x 3 months, all filled
    assert c["Q1", "Total", "All", "Sales"] == pytest.approx(100000)
    # the quarter now holds 100,000 instead of 34,840: the year changes by exactly that difference
    assert c["Year", "Total", "All", "Sales"] == pytest.approx(166600 + (100000 - 34840))
    # proportions inside the quarter are kept: Jan/North/sedan was 2000 of 34840
    assert c["Jan", "North", "sedan", "Sales"] == pytest.approx(2000 / 34840 * 100000)


def test_splash_modes(db):
    c = cube(db)
    key = ("Q1", "Total", "All", "Sales")
    c.write(key, "#24000")
    assert c["Jan", "North", "sedan", "Sales"] == 1000.0  # 24000 over 24 cells
    c.write(key, "+10%")
    assert c[key] == pytest.approx(26400)
    c.write(key, "-400")
    assert c[key] == pytest.approx(26000)


def test_numbers_are_sent_exactly_and_negatives_are_absolute(db):
    c = cube(db)
    cell = ("Jan", "North", "sedan", "Sales")
    for value in (0.1 + 0.2, 1 / 3, 1e21, 1e-7, 123456789012345.67, -50.5, -1):
        c[cell] = value
        got = c[cell]
        assert got == value, f"{value!r} came back as {got!r}"  # bit-exact, also for negatives


def test_numbers_that_are_not_finite_or_not_numbers_are_refused(db):
    c = cube(db)
    cell = ("Jan", "North", "sedan", "Sales")
    for bad in (math.nan, math.inf, -math.inf):
        with pytest.raises(neolap.InputError, match="finite"):
            c[cell] = bad
    with pytest.raises(neolap.InputError, match="bool"):
        c[cell] = True
    with pytest.raises(neolap.InputError):
        c[cell] = [1]
    with pytest.raises(neolap.InputError, match="not a number"):
        c[cell] = "abc"
    assert c[cell] == 1000.0  # nothing was written


def test_rule_and_weighted_cells_cannot_be_written(db):
    c = cube(db)
    with pytest.raises(neolap.PermissionError, match="rule"):
        c["Jan", "North", "sedan", "Profit in %"] = 1
    with pytest.raises(neolap.InputError, match="weight"):
        c["Jan", "North", "sedan", "Profit"] = 1


def test_undo_reverts_the_last_operation(db):
    c = cube(db)
    c["Q1", "Total", "All", "Sales"] = 1000
    assert c["Q1", "Total", "All", "Sales"] == pytest.approx(1000)
    assert c.undo() > 0
    assert c["Q1", "Total", "All", "Sales"] == 33840.0
    with pytest.raises(neolap.InputError, match="nothing to undo"):
        c.undo()


def test_conflicts_name_who_changed_the_cell(db):
    c = cube(db)
    seen = c.generation
    c["Jan", "North", "sedan", "Sales"] = 1  # a change after `seen`
    with pytest.raises(neolap.ConflictError) as e:
        c.write(("Q1", "North", "sedan", "Sales"), 500, expected_generation=seen)
    assert e.value.user in ("tester", "python")
    assert "Jan" in e.value.cell
    assert e.value.value == 1.0
    # with the current generation the same write goes through
    assert c.write(("Q1", "North", "sedan", "Sales"), 500, expected_generation=c.generation) > 0


def test_generation_grows_with_changes_only(db):
    c = cube(db)
    g = c.generation
    c["Jan", "North", "sedan", "Sales"] = 1000  # unchanged value
    assert c.generation == g
    c["Jan", "North", "sedan", "Sales"] = 5
    assert c.generation > g


def test_rules_text(db):
    assert "Profit in %" in cube(db).rules()
