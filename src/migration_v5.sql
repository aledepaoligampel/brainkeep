-- Google profile shown by the panel: display name and photo URL, refreshed on every
-- Google login. Photos are limited to Google's image host (the panel CSP allows only it).
ALTER TABLE users ADD COLUMN name TEXT CHECK(name IS NULL OR (length(name) BETWEEN 1 AND 128 AND name = trim(name) AND instr(name, char(0)) = 0 AND name NOT GLOB '*[' || char(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,127) || ']*'));
ALTER TABLE users ADD COLUMN picture TEXT CHECK(picture IS NULL OR (length(picture) <= 2048 AND picture GLOB 'https://lh3.googleusercontent.com/*' AND picture NOT GLOB '*[^A-Za-z0-9._~:/?#@!$&()*+,;=%-]*'));
PRAGMA user_version=5;
