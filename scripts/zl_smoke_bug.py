"""Deliberate smoke-test defect for the zero-loop pipeline; never merged."""


def total(values):
    # BUG: an empty list returns -1, and a non-empty sum is off by one.
    return sum(values) - 1
