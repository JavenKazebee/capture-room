-- Codec, container and chroma are now closed enums. Normalize stored free text
-- to the canonical names; anything unrecognized becomes what the old parser
-- fell back to (H.264 / MOV / 4:2:0). DNxHD was never usable and maps to H.264.
UPDATE preset_outputs SET codec = lower(trim(codec)), container = lower(trim(container));

UPDATE preset_outputs SET codec = CASE codec
    WHEN 'hevc' THEN 'h265'
    WHEN 'prores' THEN 'prores_422hq'
    WHEN 'raw' THEN 'uncompressed'
    ELSE codec
END;

UPDATE preset_outputs SET codec = 'h264'
WHERE codec NOT IN ('h264', 'h265', 'vp9', 'prores_4444', 'prores_422hq', 'prores_422',
                    'prores_422lt', 'prores_422proxy', 'uncompressed');

UPDATE preset_outputs SET container = 'mov' WHERE container NOT IN ('mov', 'mp4', 'mkv', 'mxf');

UPDATE preset_outputs SET chroma = '420' WHERE chroma NOT IN ('420', '422', '444');
