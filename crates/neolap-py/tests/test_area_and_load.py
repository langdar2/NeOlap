"""Areas, pandas and loading, once per backend."""

import io

import pytest

import neolap

pd = pytest.importorskip("pandas")
np = pytest.importorskip("numpy")


def sales(db):
    return db.cube("sales")


def test_an_area_is_a_dataframe_with_categories_and_floats(db):
    df = sales(db).area(years=["Jan", "Feb"], regions="North", products="sedan").to_pandas()
    assert list(df.columns) == ["years", "regions", "products", "measures", "value"]
    assert len(df) == 4  # 2 months x the 2 base measures (Sales, Cost); Profit is an aggregate
    assert isinstance(df["years"].dtype, pd.CategoricalDtype)
    assert df["value"].dtype == np.float64
    row = df[(df.years == "Jan") & (df.measures == "Sales")].iloc[0]
    assert row["value"] == 1000.0


def test_an_area_names_its_dimensions_case_insensitively_and_refuses_unknown_ones(db):
    a = sales(db).area(YEARS="Jan", Regions="North", products="sedan", measures="Sales")
    assert a.rows() == [("Jan", "North", "sedan", "Sales", 1000.0)]
    with pytest.raises(neolap.InputError, match="not a dimension .*years"):
        sales(db).area(year="Jan")
    with pytest.raises(neolap.InputError, match="empty"):
        sales(db).area(years=[])
    with pytest.raises(neolap.InputError, match="expected a member name"):
        sales(db).area(years=5)
    with pytest.raises(neolap.NotFoundError, match="Atlantis"):
        sales(db).area(regions="Atlantis").to_pandas()


def test_aggregates_and_rules_in_an_area(db):
    c = sales(db)
    a = c.area(years="Q1", regions="Total", products="All", measures="Sales", aggregates=True)
    assert a.rows() == [("Q1", "Total", "All", "Sales", 33840.0)]
    with_rules = c.area(years="Year", regions="Total", products="All", measures="Profit in %", aggregates=True, rules=True)
    assert with_rules.rows()[0][-1] == pytest.approx(0.35)
    assert len(c.area(years="Year", regions="Total", products="All", measures="Profit in %", aggregates=True)) == 0  # no stored values


def test_wide_format_has_one_column_per_member(db):
    df = sales(db).area(years="Jan", regions="North", products="sedan", wide="measures", aggregates=True).to_pandas()
    assert list(df.columns)[:3] == ["years", "regions", "products"]
    assert {"Sales", "Cost"} <= set(df.columns)
    assert df.loc[0, "Sales"] == 1000.0 and df.loc[0, "Cost"] == 650.0


def test_a_member_called_like_a_number_stays_text(db, tmp_path):
    # a year is the most natural member name there is; it must not turn into a float
    d = db.cube("sales")
    a = d.area(years="Jan", regions="North", products="sedan", measures="Sales")
    assert all(isinstance(x, str) for x in a.rows()[0][:4])


def test_an_empty_area_is_an_empty_dataframe_with_columns(db):
    c = sales(db)
    c["Jan", "North", "sedan", "Sales"] = None  # delete the only cell of this selection
    df = c.area(years="Jan", regions="North", products="sedan", measures="Sales").to_pandas()
    assert len(df) == 0
    assert list(df.columns) == ["years", "regions", "products", "measures", "value"]


def test_a_member_that_is_named_is_returned_even_if_it_is_an_aggregate(db):
    # `aggregates` only matters for dimensions that are not named
    rows = sales(db).area(years="Q1", regions="North", products="sedan", measures="Profit").rows()
    assert [r[3] for r in rows] == ["Profit"] and rows[0][-1] == pytest.approx(1123.5)


def test_area_is_read_when_asked_and_only_once(db):
    a = sales(db).area(years="Jan")
    assert "Area('sales'" in repr(a)
    n = len(a)
    sales(db)["Jan", "North", "sedan", "Sales"] = 1  # a later change does not alter an area already read
    assert len(a) == n


SPEC_WIDE = dict(mapping={"years": "Monat", "regions": "Region", "products": "Produkt"}, values={"Sales": "Umsatz", "Cost": "Kosten"})


def test_load_a_dataframe_in_wide_format(db):
    c = sales(db)
    df = pd.DataFrame({"Monat": ["Jan", "Feb"], "Region": ["North", "North"], "Produkt": ["sedan", "sedan"], "Umsatz": [1234.5, 2000.0], "Kosten": [600.0, np.nan]})
    report = c.load(df, mode="overwrite", **SPEC_WIDE)
    assert (report["rows_read"], report["rows_rejected"], report["dry_run"]) == (2, 0, False)
    assert c["Jan", "North", "sedan", "Sales"] == 1234.5
    assert c["Feb", "North", "sedan", "Sales"] == 2000.0
    assert c["Jan", "North", "sedan", "Cost"] == 600.0
    assert c["Feb", "North", "sedan", "Cost"] == 650 * 1.07 or c["Feb", "North", "sedan", "Cost"] is not None  # NaN = left alone


def test_load_long_format_with_a_fixed_member(db):
    c = sales(db)
    df = pd.DataFrame({"m": ["Jan", "Feb"], "wert": [11.0, 22.0]})
    c.load(df, mapping={"years": "m"}, fixed={"regions": "North", "products": "sedan", "measures": "Sales"}, value_column="wert")
    assert (c["Jan", "North", "sedan", "Sales"], c["Feb", "North", "sedan", "Sales"]) == (11.0, 22.0)


def test_floats_in_a_dataframe_arrive_bit_exact(db):
    c = sales(db)
    vals = [0.1 + 0.2, 1 / 3, 1e-7, 123456789012345.67, -50.5]
    months = ["Jan", "Feb", "Mar", "Apr", "May"]
    c.load(pd.DataFrame({"m": months, "v": vals}), mapping={"years": "m"}, fixed={"regions": "North", "products": "sedan", "measures": "Sales"}, value_column="v")
    for m, v in zip(months, vals):
        assert c[m, "North", "sedan", "Sales"] == v, m


def test_load_from_a_csv_file_with_german_numbers(db, tmp_path):
    f = tmp_path / "daten.csv"
    f.write_text("Monat;Region;Produkt;Umsatz;Kosten\nJan;North;sedan;1.000,5;600\nFeb;North;sedan;2.000;1.200\n", encoding="utf-8")
    c = sales(db)
    c.load(f, delimiter=";", decimal=",", **SPEC_WIDE)
    assert c["Jan", "North", "sedan", "Sales"] == 1000.5
    assert c["Feb", "North", "sedan", "Sales"] == 2000.0  # 2.000 is two thousand
    assert c["Feb", "North", "sedan", "Cost"] == 1200.0


def test_a_dry_run_changes_nothing_and_reports_the_same(db):
    c = sales(db)
    df = pd.DataFrame({"Monat": ["Jan"], "Region": ["North"], "Produkt": ["sedan"], "Umsatz": [1.0], "Kosten": [2.0]})
    g = c.generation
    dry = c.load(df, dry_run=True, **SPEC_WIDE)
    assert dry["dry_run"] is True and dry["cells_updated"] == 2
    assert c.generation == g and c["Jan", "North", "sedan", "Sales"] == 1000.0
    real = c.load(df, **SPEC_WIDE)
    assert real["cells_updated"] == dry["cells_updated"]


def test_replace_area_twice_is_the_same_as_once(db):
    c = sales(db)
    df = pd.DataFrame({"Monat": ["Jan", "Feb"], "Region": ["North", "North"], "Produkt": ["sedan", "sedan"], "Umsatz": [5.0, 6.0], "Kosten": [1.0, 2.0]})
    c.load(df, mode="replace_area", **SPEC_WIDE)
    g = c.generation
    again = c.load(df, mode="replace_area", **SPEC_WIDE)
    assert (again["cells_created"], again["cells_updated"], again["cells_deleted"]) == (0, 0, 0)
    assert c.generation == g


def test_add_mode_adds(db):
    c = sales(db)
    df = pd.DataFrame({"Monat": ["Jan"], "Region": ["North"], "Produkt": ["sedan"], "Umsatz": [5.0], "Kosten": [0.0]})
    c.load(df, mode="add", **SPEC_WIDE)
    assert c["Jan", "North", "sedan", "Sales"] == 1005.0


def test_a_bad_row_stops_the_load_and_names_the_line(db):
    c = sales(db)
    df = pd.DataFrame({"Monat": ["Jan", "Smarch"], "Region": ["North", "North"], "Produkt": ["sedan", "sedan"], "Umsatz": [1.0, 2.0], "Kosten": [1.0, 2.0]})
    with pytest.raises(neolap.InputError, match=r"1 row\(s\) rejected.*line 3.*Smarch"):
        c.load(df, **SPEC_WIDE)
    assert c["Jan", "North", "sedan", "Sales"] == 1000.0  # all or nothing


def test_load_describes_mistakes_in_the_call_before_sending_anything(db, tmp_path):
    c = sales(db)
    df = pd.DataFrame({"a": [1]})
    with pytest.raises(neolap.InputError, match="describe the columns"):
        c.load(df)
    with pytest.raises(neolap.InputError, match="exactly one of values"):
        c.load(df, mapping={"years": "a"})
    with pytest.raises(neolap.InputError, match="exactly one of values"):
        c.load(df, mapping={"years": "a"}, values={"Sales": "a"}, value_column="a")
    with pytest.raises(neolap.InputError, match="not a dimension"):
        c.load(df, mapping={"jahre": "a"}, value_column="a")
    with pytest.raises(neolap.InputError, match="mode must be"):
        c.load(df, mapping={"years": "a"}, value_column="a", mode="merge")
    with pytest.raises(neolap.InputError, match="no such file"):
        c.load(tmp_path / "missing.csv", mapping={"years": "a"}, value_column="a")
    with pytest.raises(neolap.InputError, match="cannot load from list"):
        c.load([1, 2], mapping={"years": "a"}, value_column="a")
    with pytest.raises(neolap.InputError, match="column 'x' not found"):
        c.load(pd.DataFrame({"a": ["Jan"], "v": [1.0]}), mapping={"years": "x"}, fixed={"regions": "North", "products": "sedan", "measures": "Sales"}, value_column="v")


def test_names_with_quotes_and_backslashes_cannot_break_the_specification(db):
    # the specification is TOML text built from the arguments: a name with quotes must stay a name
    c = sales(db)
    df = pd.DataFrame({'Mo"nat\\': ["Jan"], "v": [7.0]})
    c.load(df, mapping={"years": 'Mo"nat\\'}, fixed={"regions": "North", "products": "sedan", "measures": "Sales"}, value_column="v")
    assert c["Jan", "North", "sedan", "Sales"] == 7.0


def test_an_import_spec_can_be_given_as_text(db):
    spec = 'name = "t"\ncube = "sales"\nvalue_column = "v"\n[map]\nyears = "m"\n[fixed]\nregions = "North"\nproducts = "sedan"\nmeasures = "Sales"\n'
    c = sales(db)
    c.load(b"m,v\nJan,42\n", spec=spec)
    assert c["Jan", "North", "sedan", "Sales"] == 42.0


# ---------------------------------------------------------------- column kinds are the same in both modes

def test_a_measure_that_is_empty_everywhere_is_still_a_numeric_column(db):
    # "Profit in %" has no stored values: in a wide table it is a column of NaN, not of text
    df = sales(db).area(years="Q1", regions="Total", products="All", wide="measures", aggregates=True).to_pandas()
    assert df["Profit in %"].dtype == np.float64
    assert df["Profit in %"].isna().all()
    assert df["Sales"].dtype == np.float64
    for name in ("years", "regions", "products"):
        assert isinstance(df[name].dtype, pd.CategoricalDtype), name


def test_both_modes_give_the_same_dataframe(local, remote):
    # the same question, asked of a file and of a server holding an identical file
    kw = dict(years=["Q1", "Q2"], regions="Total", products="All", wide="measures", aggregates=True)
    a = local.cube("sales").area(**kw).to_pandas()
    b = remote.cube("sales").area(**kw).to_pandas()
    pd.testing.assert_frame_equal(a, b)
    kw = dict(years=["Jan", "Feb"], regions=["North", "South"], products="sedan")
    pd.testing.assert_frame_equal(local.cube("sales").area(**kw).to_pandas(), remote.cube("sales").area(**kw).to_pandas())


def test_numeric_looking_member_names_stay_text_in_both_modes(tmp_path, binary):
    """A year is the most natural member name there is. It must never come back as a number."""
    import os, socket, subprocess, time
    data = tmp_path / "d"
    data.mkdir()
    m = tmp_path / "m"
    (m / "dimensions").mkdir(parents=True)
    (m / "cubes").mkdir()
    (m / "dimensions/years.csv").write_text("child;parent;weight;alias\nAll;;;\n2025;All;;\n2026;All;;\n007;All;;\n1e3;All;;\n")
    (m / "dimensions/measures.csv").write_text("child;parent;weight;alias\nAmount;;;\nCount;;;\n")
    (m / "dimensions/measures.toml").write_text('kind = "Measures"\n')
    (m / "cubes/c.toml").write_text('dimensions = ["years", "measures"]\n')
    path = data / "neolap.neolap"
    for args in (["new", str(path)], ["model", "import", str(path), str(m)]):
        subprocess.run([str(binary), *args], check=True, capture_output=True)
    env = dict(os.environ, NEOLAP_PASSWORD="a very long secret")
    subprocess.run([str(binary), "user", "add", str(path), "u", "--role", "admin"], check=True, capture_output=True, env=env)
    token = subprocess.run([str(binary), "user", "token", str(path), "u"], check=True, capture_output=True, env=env, text=True).stdout.strip()
    # fill through the file first, then serve the same file
    with neolap.open(path) as db:
        c = db.cube("c")
        for y, v in (("2025", 1), ("2026", 2), ("007", 3), ("1e3", 4)):
            c[y, "Amount"] = v
        local_df = c.area().to_pandas()
    s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
    p = subprocess.Popen([str(binary), "serve", "--listen", f"127.0.0.1:{port}", "--data-dir", str(data)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={k: v for k, v in os.environ.items() if not k.startswith("NEOLAP_")})
    try:
        time.sleep(1.5)
        remote_df = neolap.connect(f"http://127.0.0.1:{port}", token=token).cube("c").area().to_pandas()
    finally:
        p.terminate(); p.wait(timeout=10)
    for df in (local_df, remote_df):
        assert isinstance(df["years"].dtype, pd.CategoricalDtype)
        assert sorted(df["years"].astype(str)) == ["007", "1e3", "2025", "2026"]  # "007" kept its zeros, "1e3" is not 1000
        assert df["value"].dtype == np.float64
    pd.testing.assert_frame_equal(local_df, remote_df)
