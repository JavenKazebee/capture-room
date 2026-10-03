-- MXF was never recordable (mxfmux rejected every codec), and VP9 only works
-- in MKV. Move stored outputs to the nearest container that records.
UPDATE preset_outputs SET container = 'mkv' WHERE codec = 'vp9' AND container IN ('mov', 'mp4', 'mxf');
UPDATE preset_outputs SET container = 'mov' WHERE container = 'mxf';
UPDATE preset_outputs SET container = 'mov'
WHERE container = 'mp4' AND codec IN ('prores_4444', 'prores_422hq', 'prores_422', 'prores_422lt',
                                      'prores_422proxy', 'uncompressed');
