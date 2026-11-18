# Test data

`fixture.copc.laz` is a synthetic COPC file made by `generate_fixture.py`
(Python standard library plus `pdal` on PATH; it was made with PDAL 2.10.2
via its `readers.text` → `writers.copc` pipeline). No real survey data is in
it, so there are no licensing or privacy concerns.

- 7,739 points, LAS 1.4 point format 7 (RGB), scale 0.01 m
- Bounds X 500000–500100, Y 5400000–5400060, Z 100–125 (exact: the ground's
  corners and the tallest tree's tip are placed on them)
- Classes: 2 ground 3,969; 3 low vegetation 400; 5 high vegetation 2,465;
  6 building 905
- Ground sloping up to the east, a building in the north-west with a hedge
  south of it, four cone trees in the south-east. It is asymmetric so a
  mirrored or rotated render fails the tests.

The integration tests (`tests/common/mod.rs`, `expected`) assert these
numbers, and `tests/golden/*.png` are renders of this exact file.

`stac/` holds hand-written STAC catalogs pointing at the fixture, served by
the test server (`tests/common/server.rs`) for `tests/stac_client.rs` and
`tests/ui_connectors.rs`:

- `stac/static/`: a static catalog → collection → three items: Site A with one
  COPC asset (the fixture), Site B with none, Site C with two (the fixture,
  and an `s3://` link that can't be opened).
- `stac/api/`: a fake STAC API (landing page, `/collections`, three pages of
  items; the last item, "Late flight", has a PNG thumbnail). Directories are served as their `index.json`, and the server writes
  its own address in place of `{{base}}`.

PDAL's COPC writer doesn't lay out the octree identically on every run, so
regenerating gives a file with the same points but different bytes and node
contents. That changes renders slightly: after regenerating, update the
goldens (see `tests/golden_render.rs`) and say why in the commit.
