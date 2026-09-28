# SheetJS Community Edition 0.20.3

- Source: https://cdn.sheetjs.com/xlsx-0.20.3/xlsx-0.20.3.tgz (official SheetJS CDN; 0.20.x is not on the npm registry)
- Files taken from the tarball: `package/dist/xlsx.full.min.js`, `package/LICENSE` (Apache-2.0)
- Tarball sha512 (npm integrity): `sha512-oLDq3jw7AcLqKWH2AhCpVTZl8mf6X2YReP+Neh0SJUzV/BdZYjth94tG5toiMB1PPrYtxOCfaoUCkvtuH+3AJA==`
- Tarball sha256: `8dc73fc3b00203e72d176e85b50938627c7b086e607c682e8d3c22c02bb99fe8`
- `xlsx.full.min.js` sha256: `cc015130aa8521e7f088f88898eba949ccdcbfb38df0bd129b44b7273c3a6f41`

SheetJS does not publish a checksum file. The hashes above were computed from two separate
downloads (identical), and the sha512 matches the integrity recorded for this URL in public
npm/pnpm lockfiles of unrelated projects.

Replaces 0.18.5 (CVE-2023-30533 prototype pollution, CVE-2024-22363 ReDoS).
