"""Turning column data into pandas, kept apart so that `import neolap` needs no pandas."""

from __future__ import annotations


def _pandas():
    try:
        import numpy as np
        import pandas as pd
    except ImportError as e:  # pragma: no cover - depends on the environment
        raise ImportError("pandas and numpy are needed for DataFrames: pip install 'neolap[pandas]'") from e
    return np, pd


def to_pandas(columns):
    """A `DataFrame` from native `Columns` (no Python object per cell) or the remote text columns.

    Dimension columns become `category`, so a million rows cost four bytes per dimension and row.
    Empty cells are NaN.
    """
    np, pd = _pandas()
    data = {}
    if hasattr(columns, "buffer"):  # native: raw little-endian buffers
        for i, name in enumerate(columns.headers):
            buf = columns.buffer(i)
            if columns.is_numeric(i):
                data[name] = np.frombuffer(buf, dtype="<f8").copy()
            else:
                codes = np.frombuffer(buf, dtype="<u4").astype("int32")
                data[name] = pd.Categorical.from_codes(codes, categories=columns.names(i))
        return pd.DataFrame(data)
    # remote: CSV text columns; the kind of a column was decided by its position, not by its content
    for c, name in enumerate(columns.headers):
        col = [r[c] for r in columns._rows]
        if columns.numeric[c]:
            data[name] = np.array([float(v) if v != "" else np.nan for v in col], dtype="<f8")
        else:
            # categories in the order they first appear, which is the order of the model (a plain
            # `pd.Categorical(col)` would sort them alphabetically: Apr, Aug, Dec ... instead of Jan, Feb, Mar)
            data[name] = pd.Categorical(col, categories=list(dict.fromkeys(col)))
    return pd.DataFrame(data)
