# Vendored three.js

- Version: **0.187.0** (r187), MIT license (see `LICENSE`).
- Source: the official npm tarball <https://registry.npmjs.org/three/-/three-0.187.0.tgz>
  (integrity `sha512-Qiu7R+Cmh8CstZsY1uJOSWnVlfo7ZtNLV7mF8Jltk3LB3AtDu9cRZlcnRKkAk8Y4z6KKw35z7Y7Urd7jWY8etA==`).
- Files are copied unmodified:
  - `build/three.module.js` → `three.module.js` (imports `./three.core.js`)
  - `build/three.core.js` → `three.core.js`
  - `examples/jsm/controls/OrbitControls.js` → `addons/controls/OrbitControls.js`
  - `examples/jsm/environments/RoomEnvironment.js` → `addons/environments/RoomEnvironment.js`
- The addons import the bare specifier `three`; `index.html` maps it (and `three/addons/`) to
  this folder with an import map, so no build step is needed.

r187 no longer ships a `three.module.min.js` build, so the unminified ES modules are used. They
are only fetched when the 3D case view is opened for the first time.

To upgrade: `npm pack three@<version>`, check the tarball integrity against
`npm view three@<version> dist.integrity`, copy the same files over, and update this README.
