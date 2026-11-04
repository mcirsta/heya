-- Derived map tiles stay in the account archive and follow its encryption mode.
CREATE TABLE IF NOT EXISTS location_map_tiles (
    zoom INTEGER NOT NULL,
    x INTEGER NOT NULL,
    y INTEGER NOT NULL,
    bytes BLOB NOT NULL,
    fetched INTEGER NOT NULL,
    PRIMARY KEY(zoom,x,y)
);
