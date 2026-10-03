-- Chroma subsampling for H.264/H.265 outputs: "420" | "422" | "444".
ALTER TABLE preset_outputs ADD COLUMN chroma TEXT NOT NULL DEFAULT '420';
