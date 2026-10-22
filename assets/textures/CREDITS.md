# Texture credits

| File | Source | License |
|---|---|---|
| `earth_day.jpg` | NASA Visible Earth, Blue Marble (land_shallow_topo) | Public domain |
| `sun.jpg`, `mercury.jpg`, `venus.jpg`, `mars.jpg`, `jupiter.jpg`, `saturn.jpg`, `uranus.jpg`, `neptune.jpg`, `earth_night.jpg`, `earth_clouds.jpg`, `saturn_ring.rgba` | [Solar System Scope](https://www.solarsystemscope.com/textures/), based on NASA mission data | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |

The maps were downsampled to 1024×512 and re-encoded as JPEG. `saturn_ring.rgba` is the Solar
System Scope ring texture averaged into a 512-sample radial profile (RGBA); `src/textures.rs`
maps it to radii calibrated on the outer edges of the B and A rings.
