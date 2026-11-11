# UI v2 fonts: Alegreya and Fira Sans

The UI v2 text stack (`crates/client/ui2-text`) is measured against Chrome with two OFL font families. The files
are vendored unchanged from the google/fonts repository (commit c30b221beb0c1da59d563feb91ec8656a1fa972a):

- `third_party/fonts/alegreya/Alegreya[wght].ttf`: Copyright 2011 The Alegreya Project Authors
  (https://github.com/huertatipografica/Alegreya), SIL Open Font License 1.1, text in
  `third_party/fonts/alegreya/OFL.txt`.
- `third_party/fonts/fira-sans/FiraSans-Regular.ttf`, `FiraSans-Medium.ttf`: Copyright 2012-2015 The Mozilla
  Foundation and Telefonica S.A., SIL Open Font License 1.1, text in `third_party/fonts/fira-sans/OFL.txt`.

The fonts are not game content. They are used by the `ui2-text` tests and may be embedded in UI skins; the
OFL permits redistribution and embedding with the license text kept.