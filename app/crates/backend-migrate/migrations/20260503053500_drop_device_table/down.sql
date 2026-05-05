CREATE TABLE device (
  device_id text NOT NULL,
  user_id text NOT NULL REFERENCES app_user(user_id),
  jkt text NOT NULL,
  public_jwk text NOT NULL,
  status text NOT NULL,
  label text,
  created_at timestamptz NOT NULL DEFAULT now(),
  last_seen_at timestamptz,
  PRIMARY KEY (device_id, public_jwk)
);

CREATE UNIQUE INDEX device_device_id_unique ON device(device_id);
CREATE UNIQUE INDEX device_public_jwk_unique ON device(public_jwk);
CREATE INDEX device_user_id_idx ON device(user_id);
CREATE INDEX device_jkt_idx ON device(jkt);

CREATE TABLE signing_key (
  kid TEXT PRIMARY KEY,
  private_key_pem TEXT NOT NULL,
  public_key_jwk JSONB NOT NULL,
  algorithm TEXT NOT NULL DEFAULT 'RS256',
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ,
  is_active BOOL NOT NULL DEFAULT TRUE
);

CREATE INDEX idx_signing_key_active ON signing_key(is_active) WHERE is_active = TRUE;
