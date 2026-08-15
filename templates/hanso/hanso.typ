// ============================================================================
// Hanso brand template for documents (Typst) – self-contained, single file.
//
// Reproduces the Word "master.dotx" look: retro stripe corner sweeps, Figtree
// headings/body, roman/alpha heading scheme, standfirst, and the 3-column
// address / contact / bank footer. Light + dark themes from the website tokens
// (hanso_web/assets/css/hanso_theme.css). The brand SVGs (logo, stripe sweep,
// contact icons) are embedded below as minified strings – no assets/ needed;
// the canonical artwork lives in assets/.
//
// Requires: Typst 0.13+ and the Figtree font family (installed or --font-path).
//
// Usage:
//   #import "hanso.typ": *
//   #show: hanso-doc.with(title: "...", author: "...", date: datetime(...))
//   dark mode:     #show: hanso-doc.with(theme: dark-theme, ...)
//   simple footer: #show: hanso-doc.with(footer-style: "simple", ...)
//                  -> one line: support email · website · confidentiality · social
//                     (for internal docs: change logs, training material). The full
//                     3-column address/contact/bank footer is the default.
//
//   = Chapter          -> new page, numbered I. II. III., uppercase
//   == Section         -> numbered a. b. c.   (=== -> 1. 2. 3.)
//   #quote(block: true)[...]                  -> standfirst under a chapter title
//   #figure(image(..), caption: [..])         -> numbered "Figure N – ..."
//   #figure(table(..), caption: [..])         -> branded table, "Table N – ..."
//   #figure(hanso-barchart((("label", value, "display", colour), ..)), caption: [..]) -> bar chart
// ============================================================================

// --- Brand palette ----------------------------------------------------------
// Official token names + hex from the website theme. The third logo colour is
// plain white (Typst's built-in `white`).
#let hanso-black = rgb("#191D1C") // --color-text
#let hanso-white = rgb("#F2EEE4") // --color-background (the beige)
#let sunflower-yellow = rgb("#FBCB36") // spot 1
#let gerbera-red = rgb("#F85A1A") // spot 2
#let crimson-red = rgb("#EE1C26") // spot 3
#let wine-red = rgb("#B91A40") // spot 4
#let dark-maroon = rgb("#661F41") // spot 5
#let stripes = (sunflower-yellow, gerbera-red, crimson-red, wine-red, dark-maroon)

// Dark palette (website [data-theme="hanso-dark"]).
#let dm-navy = rgb("#001B2E") // --color-background
#let dm-blue = rgb("#1F5780") // spot 1
#let dm-teal = rgb("#4F959C") // spot 2
#let dm-light-teal = rgb("#97D3C1") // spot 3
#let dm-light-yellow = rgb("#F6F7D5") // spot 4
#let dm-purple = rgb("#761E48") // spot 5
#let dark-stripes = (dm-blue, dm-teal, dm-light-teal, dm-light-yellow, dm-purple)

// --- Themes -------------------------------------------------------------------
// A theme is a dictionary of colours: page background, foreground (text – the
// logo and icons are recoloured to match it), the accent (standfirst, figure
// labels, table header rule), the footer hairline, and the five sweep stripes.
#let light-theme = (
  bg: hanso-white,
  fg: hanso-black,
  accent: wine-red,
  rule: hanso-black,
  stripes: stripes,
)
#let dark-theme = (
  bg: dm-navy,
  fg: hanso-white,
  accent: dm-light-teal, // readable accent on navy (spot 3)
  rule: hanso-white,
  stripes: dark-stripes,
)

// --- Embedded brand artwork ---------------------------------------------------
// Minified copies of the SVGs in assets/, with colours replaced by tokens:
// {INK} = the single ink colour of logo/icons, {S1}..{S5} = the sweep stripes.
#let _svg-logo = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1922.538 1549.959'><g><path fill='{INK}' d='m481.404,1012.85v102.314h-140.127v-102.314c0-2.704-2.192-4.895-4.895-4.895h-36.08c-2.704,0-4.895,2.192-4.895,4.895v246.173c0,2.704,2.192,4.895,4.895,4.895h36.08c2.704,0,4.895-2.192,4.895-4.895v-103.576h140.127v103.576c0,2.704,2.192,4.895,4.895,4.895h36.08c2.704,0,4.895-2.192,4.895-4.895v-246.173c0-2.704-2.192-4.895-4.895-4.895h-36.08c-2.704,0-4.895,2.192-4.895,4.895Z'/><path fill='{INK}' d='m1381.123,1068.177v135.52c0,33.26,26.962,60.222,60.222,60.222h125.565c33.26,0,60.222-26.962,60.222-60.222v-135.52c0-33.26-26.962-60.222-60.222-60.222h-125.565c-33.26,0-60.222,26.962-60.222,60.222Zm45.87,129.769v-124.019c0-14.187,11.501-25.689,25.689-25.689h102.89c14.187,0,25.689,11.501,25.689,25.689v124.019c0,14.187-11.501,25.689-25.689,25.689h-102.89c-14.187,0-25.689-11.501-25.689-25.689Z'/><path fill='{INK}' d='m1262.194,1115.794h-67.787c-18.316,0-33.915-14.242-34.565-32.546-.683-19.217,14.692-35.01,33.756-35.01h126.018c2.704,0,4.895-2.192,4.895-4.895v-30.494c0-2.704-2.192-4.895-4.895-4.895h-126.018c-43.988,0-79.648,33.159-79.648,74.062s35.66,74.062,79.648,74.062h67.787c18.316,0,33.915,14.242,34.566,32.546.683,19.217-14.692,35.01-33.756,35.01h0s-135.232,0-135.232,0c-2.704,0-4.895,2.192-4.895,4.895v30.494c0,2.704,2.192,4.895,4.895,4.895h135.232c43.988,0,79.648-33.158,79.648-74.062s-35.66-74.062-79.648-74.062Z'/><path fill='{INK}' d='m806.294,1258.99l-93.421-243.025c-1.835-4.823-6.458-8.011-11.618-8.011h-38.07c-5.16,0-9.784,3.188-11.618,8.011l-93.421,243.025c-.42,1.099-.305,2.281.308,3.27.62,1.001,2.178,1.763,3.355,1.763h33.528c5.906,0,7.948-1.094,10.038-6.617l76.866-208.528,76.823,208.528c2.09,5.523,4.132,6.617,10.038,6.617h33.528c1.177,0,2.735-.762,3.355-1.763.614-.989.728-2.17.308-3.27Z'/><path fill='{INK}' d='m1067.005,1007.955h-36.204c-2.801,0-5.072,2.271-5.072,5.072v177.846l-141.586-174.182h0c-5.44-6.443-8.133-8.733-14.612-8.736h-25.887c-3.229,0-5.848,2.618-5.848,5.848v245.149c0,2.801,2.271,5.071,5.071,5.071h36.204c2.801,0,5.071-2.271,5.071-5.071v-177.846l141.587,174.182c5.44,6.443,8.133,8.733,14.613,8.736h26.663c2.801,0,5.072-2.271,5.072-5.072v-245.925c0-2.801-2.271-5.072-5.072-5.072Z'/></g><g><g><rect fill='{INK}' x='928.031' y='715.114' width='52.793' height='52.793' rx='4.52' ry='4.52'/><path fill='{INK}' d='m1097.066,851.924c4.09,0,6.594-4.487,4.446-7.968l-22.846-37.027c-1.84-2.982-5.094-4.798-8.598-4.798h-231.283c-3.504,0-6.758,1.816-8.598,4.798l-22.846,37.027c-2.148,3.481.356,7.968,4.446,7.968h285.278Z'/><path fill='{INK}' d='m1033.839,459.932h-158.072c-30.979,0-56.092,25.113-56.092,56.092v108.771c0,30.979,25.113,56.092,56.092,56.092h157.322c30.979,0,56.092-25.113,56.092-56.092v-109.521c0-30.565-24.777-55.342-55.342-55.342Zm-14.764,172.344h-130.318c-7.564,0-13.697-6.132-13.697-13.697v-96.339c0-7.564,6.132-13.697,13.697-13.697h130.318c7.564,0,13.697,6.132,13.697,13.697v96.339c0,7.564-6.132,13.697-13.697,13.697Z'/></g><path fill='{INK}' d='m1270.62,547.513l-136.878-237.075c-8.754-15.162-24.932-24.502-42.44-24.502h-273.751c-17.508,0-33.685,9.34-42.44,24.502l-136.878,237.075c-8.726,15.115-8.726,33.892,0,49.006l132.796,230.627c2.027,3.52,7.077,3.597,9.211.141l19.43-31.481-.012.005,4.874-7.9c2.119-3.432,2.181-7.735.165-11.228l-116.203-201.893c-.988-1.711-.988-3.837,0-5.547l130.564-226.144c.99-1.715,2.822-2.772,4.802-2.772h261.131c1.981,0,3.812,1.057,4.802,2.772l130.564,226.144c.988,1.71.988,3.836,0,5.547l-116.203,201.893c-2.015,3.493-1.953,7.795.165,11.228l4.874,7.9-.012-.005,19.43,31.481c2.134,3.456,7.184,3.379,9.211-.141l132.796-230.627c8.726-15.114,8.726-33.891,0-49.006Z'/></g></svg>"
#let _svg-sweep = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 4805.039 637.723'><g><path d='M0 595.951v-56.769h2164.366c39.121 0 77.657-10.141 111.444-29.324 33.887-19.244 62.359-47.126 82.34-80.632l120.316-201.812c33.858-56.807 95.983-92.096 162.131-92.096l2164.442.181v56.769l-2164.405-.181c-46.271 0-89.724 24.674-113.401 64.392l-120.316 201.815c-25.002 41.932-60.648 76.831-103.084 100.924-42.294 24.031-90.513 36.733-139.442 36.733H0Z' style='fill:{S4}'/><path d='M0 502.4v-56.768h2164.393c46.304-.002 89.757-24.642 113.401-64.303l120.317-201.814c25.003-41.934 60.648-76.832 103.084-100.924 42.396-24.09 90.616-36.823 139.443-36.823l2164.4.18v56.77l-2164.378-.182c-39.018 0-77.555 10.171-111.443 29.414-33.888 19.244-62.36 47.127-82.34 80.632l-120.315 201.813c-33.824 56.751-95.949 92.005-162.131 92.005H0Z' style='fill:{S2}'/><path d='m4805.039 93.717-2164.456-.18c-39.859-.007-79.246 10.382-113.913 30.065-34.646 19.676-63.749 48.182-84.166 82.42l-1.004 1.685-119.312 200.13c-32.976 55.327-93.488 89.586-157.895 89.565H0v46.782h2164.286c39.859.007 79.246-10.292 113.913-29.975 34.646-19.676 63.748-48.182 84.166-82.419l1.004-1.685 119.312-200.13c32.976-55.328 93.488-89.676 157.895-89.655l2164.464.18V93.718Z' style='fill:{S3}'/><path d='m4805.039 187.267-2164.405-.18c-48.023-.014-93.114 25.595-117.697 66.833l-.336.563-.003-.002-91.187 152.958-27.453 46.049-1.338 2.246c-24.555 41.182-59.569 75.468-101.257 99.136-41.681 23.682-89.066 36.088-137.013 36.081H0v46.772h2164.338c56.034.027 111.396-14.503 160.119-42.17 48.716-27.674 89.641-67.734 118.334-115.867l1.674-2.809h.003l2.067-3.466 116.572-195.54c16.19-27.161 45.895-44.024 77.512-44.017l2164.419.18v-46.768Z' style='fill:{S5}'/><path d='M4805.039.173 2640.69 0c-56.034-.028-111.396 14.589-160.12 42.256-48.716 27.674-89.641 67.733-118.334 115.867l-1.675 2.809h-.003l-118.639 199.006c-16.19 27.161-45.895 43.934-77.512 43.927H0v46.768h2164.393c48.023.014 93.114-25.505 117.697-66.742l.336-.563.003.002 90.894-152.467 27.746-46.54 1.338-2.246c24.555-41.182 59.569-75.468 101.257-99.136 41.681-23.682 89.066-36.178 137.013-36.171l2164.363.18V.173Z' style='fill:{S1}'/></g></svg>"
#let _svg-ic-phone = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 75.543 67.423'><g><path fill='{INK}' d='m52.936,67.423h-30.329c-2.651,0-5.122-1.427-6.448-3.723L.994,37.434c-1.325-2.296-1.325-5.149,0-7.445L16.159,3.723c1.326-2.296,3.797-3.723,6.448-3.723h30.329c2.651,0,5.122,1.427,6.448,3.723l15.165,26.266c1.325,2.296,1.325,5.149,0,7.445l-15.165,26.267c-1.326,2.296-3.797,3.723-6.448,3.723ZM22.607,4c-1.227,0-2.37.66-2.983,1.723L4.459,31.988c-.614,1.062-.614,2.383,0,3.445l15.165,26.267c.613,1.062,1.757,1.723,2.983,1.723h30.329c1.227,0,2.37-.66,2.983-1.723l15.165-26.267c.614-1.062.614-2.383,0-3.445l-15.165-26.266c-.613-1.062-1.757-1.723-2.983-1.723h-30.329Z'/><path fill='{INK}' d='m67.821,24.339h-17.959c-1.323,0-2.546-.718-3.191-1.874l-3.525-6.326h-10.749l-3.525,6.325c-.646,1.157-1.868,1.875-3.191,1.875H7.72v-4h17.755l3.525-6.325c.644-1.156,1.867-1.875,3.191-1.875h11.157c1.324,0,2.548.719,3.191,1.876l3.525,6.324h17.755v4Z'/><rect fill='{INK}' x='28.45' y='30.594' width='4.594' height='4.594'/><rect fill='{INK}' x='35.474' y='30.594' width='4.594' height='4.594'/><rect fill='{INK}' x='42.499' y='30.594' width='4.594' height='4.594'/><rect fill='{INK}' x='28.45' y='37.984' width='4.594' height='4.594'/><rect fill='{INK}' x='35.474' y='37.984' width='4.594' height='4.594'/><rect fill='{INK}' x='42.499' y='37.984' width='4.594' height='4.594'/><rect fill='{INK}' x='28.45' y='45.374' width='4.594' height='4.594'/><rect fill='{INK}' x='35.474' y='45.374' width='4.594' height='4.594'/><rect fill='{INK}' x='42.499' y='45.374' width='4.594' height='4.594'/><rect fill='{INK}' x='35.474' y='52.765' width='4.594' height='4.594'/></g></svg>"
#let _svg-ic-mail = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 75.543 67.423'><g><path fill='{INK}' d='m52.936,67.423h-30.329c-2.651,0-5.122-1.427-6.448-3.723L.994,37.434c-1.325-2.296-1.325-5.149,0-7.445L16.159,3.723c1.326-2.296,3.797-3.723,6.448-3.723h30.329c2.651,0,5.122,1.427,6.448,3.723l15.165,26.266c1.325,2.296,1.325,5.149,0,7.445l-15.165,26.267c-1.326,2.296-3.797,3.723-6.448,3.723ZM22.607,4c-1.227,0-2.37.66-2.983,1.723L4.459,31.988c-.614,1.062-.614,2.383,0,3.445l15.165,26.267c.613,1.062,1.757,1.723,2.983,1.723h30.329c1.227,0,2.37-.66,2.983-1.723l15.165-26.267c.614-1.062.614-2.383,0-3.445l-15.165-26.266c-.613-1.062-1.757-1.723-2.983-1.723h-30.329Z'/><rect fill='{INK}' x='50.276' y='11.125' width='4.594' height='4.594'/><path fill='{INK}' d='m37.767,38.037c-1.92,0-3.84-.493-5.553-1.479L7.687,22.401l2-3.465,24.523,14.155c2.195,1.262,4.92,1.262,7.111,0l24.535-14.155,2,3.465-24.538,14.155c-1.712.987-3.632,1.48-5.552,1.48Z'/></g></svg>"
#let _svg-ic-web = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 76.295 67.629'><g><path fill='{INK}' d='m75.521,30.918L59.342,2.896c-1.035-1.792-2.947-2.896-5.016-2.896H21.969c-2.069,0-3.981,1.104-5.016,2.896L.774,30.918c-1.031,1.786-1.031,4.006,0,5.792l15.696,27.26c.037.063.083.114.134.159l.348.604c1.035,1.792,2.947,2.896,5.016,2.896h32.357c2.069,0,3.981-1.104,5.016-2.896l.348-.603c.051-.044.098-.095.134-.159l15.696-27.26c1.031-1.786,1.031-4.006,0-5.792Zm-11.018-8.055c.032-.693.287-1.112.669-1.626l1.868,3.235c-.26-.006-.535-.023-.866-.089-1.111-.222-1.769.603-1.671-1.52Zm-7.221,37.262l-.59,1.025-.082.072-.256.443-.338.586c-.448.775-1.282,1.257-2.177,1.257h-19.825c-.015-.528-.004-1.033.123-1.449.266-.87.764-1.537,1.292-2.337,2.126-3.22.343-3.206-2.365-4.648-.991-.527-1.588-1.572-2.544-2.126-.789-.457-2.179-.346-2.773-1.002-.722-.797-.436-2.558-1.284-3.187-.809-.6-2.422-.1-3.278-.665-.942-.622-1.28-2.335-2.312-2.733-.675-.261-2.234-.145-3.004-.297-1.111-.22-1.955-.354-2.725-.326-2.102-.231-2.329,1.208-3.288.804-.586-.247-1.756-1.436-2.578-2.324l-4.691-8.146c-.448-.775-.448-1.739,0-2.514l2.179-3.774c.245-.035.481-.064.648-.073.454-.025.732.126,1.489.681.757.555,0,2.927.873,4.159.061.086.118.15.172.201.935,1.108,1.702,2.216,2.473-.563.273-.984-.304-3.336.321-4.111.769-.952,2.995-.795,3.946-1.592.86-.721,1.117-2.663,2.061-3.212.6-.349,2.156-.238,2.918-.357,3.355-.521,3.313-2.006,1.756-4.731.807-.293,2.888-.485,3.584.143,1.058.955.097,2.615,1.792,2.939,1.349.258,1.721-.93,1.684-2.123-.018-.588-.53-1.183-.589-1.784-.077-.782.381-1.631.267-2.407-.401-2.737-3.302-1.418-5.106-2.718-1.293-.932-1.433-2.02-3.394-1.241-.015.006-.028.015-.043.022-.159.077-.789.334-1.764.139-1.136-.227-.379-.278-1.615-1.009-.53-.314-.801-.477-.942-.605l1.55-2.684c.117.038.226.056.351.113,1.009.458.631.527,1.06.325.429-.202.505-.454.808-.606.303-.151,1.11.126,2.709.274.33.053.648.168.939.398,1.424,1.127.398,1.484,2.383,1.902,2.364.498,4.131-.869,2.632-2.974-.531-.746-1.676-2.056-2.508-3.168h6.593c.093.114.183.232.241.382.494,1.264-.616,2.105-.974,3.089-.401,1.105.32,2.319,1.476,2.702,1.543.512,1.773-1.017,2.873-1.828,1.063-.784,1.947-.125,3.138-.609.79-.321,1.194-1.437,1.976-1.918.463-.285,1.327-.301,1.656-.706.195-.239.179-.694.199-1.111h9.44c.895,0,1.73.482,2.177,1.257l1.58,2.737c-.68.633-1.329,1.382-1.905,1.895-1.636,1.151-1.834.789-1.343,2.406.421,1.387.986,1.649,2.275,1.787,2.337.251,2.26-1.982,2.622-3.234l1.581,2.738c-.254.9-.633,1.722-.977,2.109-1.808,2.038-4.245-.352-6.199.815-.775.463-.778,2.604-2.071,2.124-.266-.099-.486-2.157-.74-2.781-.717-1.761-1.685-4.122-3.686-2.823-.577.374-2.512,3.293-2.565,3.969-.097,1.232.124.974,1.082.945,1.578-.049,1.267-1.218,2.709-.033.348.286-.179,1.21.238,1.512.323.234,1.625,0,2.055,0-.361.594-.83,1.202-1.019,1.857-.11.383.151,1.536-.128,1.798-.327.307-.988.408-2.12,0-1.132-.408-1.451-.83-2.242.211-.367.482-.704,2.532-.568,3.129.14.614.921.969.917,1.084-.009.228.838,1.011.71,1.602-.158.732-1.564,1.15-1.858,1.858-.844,2.031.552,1.873-1.363,3.35-1.45,1.117-2.152,1.218-2.508,2.787-.324,1.426.13,2.825-.334,4.228-.603,1.818-1.513,2.229-.216,3.707.882,1.004,2.39,1.497,3.371,2.417,1.403,1.316,1.452,2.619,3.473,2.579,1.284-.026,2.408-1.288,3.591-1.144,1.258.153,2.123,1.686,3.254,2.212,2.17,1.01,1.128-.007,1.671,2.694.311,1.546,1.621,2.701,2.019,4.208.17.644.098,1.319-.041,2.004Zm5.418-29.639c-.934.136-1.439.669-2.347.91-1.052.278-2.641.041-3.661-.694-.777-.561-.746-1.557-1.463-2.102-1.64-1.248-6.558-.193-8.072.678-1.309-1.4,2.331-1.651,3.045-2.077.997-.595,1.375-.987,2.261-1.957,1.952-2.137,3.413-2.005,5.683-.366,1.824,1.316,3.735.425,5.379,1.472,1.109.707,1.334,1.985,2.581,2.235.757.152,2.069-.481,3-.531l1.254,2.172c-.048.074-.094.15-.155.215-.79.834-1.957.413-3.013.343-1.412-.093-3.105-.499-4.493-.297Zm8.441,5.57l-1.516,2.633c-.023-.033-.053-.061-.074-.096-.536-.892-.134-1.363-.435-2.451-.21-.761-1.077-1.628-1.527-2.262-.159-2.382,1.217-1.199,2.004-.367.457.483,1.237,1.347,1.501,1.979.069.166.063.363.048.563Zm-30.161-24.576l2.686-.905c.299-.101.241-.54-.074-.56l-2.617-.168c-.158-.01-.295.11-.306.269l-.069,1.073c-.013.204.185.357.379.291Zm-27.972,26.695c-.936-.079-3.014-.679-3.104-.576-.665,1.258-.372,1.802.98,1.56.185-.062.41-.109.708-.101.704.018,2.175,1.804,2.848.559.893-1.652-.936-1.399-1.431-1.441Z'/></g></svg>"
#let _svg-ic-linkedin = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 27.427 27.428'><g><g><g><path fill='{INK}' fill-rule='evenodd' d='m9.024,22.281h-3.538v-11.423h3.538v11.423Zm-1.786-12.919c-1.156,0-2.092-.944-2.092-2.108s.937-2.108,2.092-2.108,2.092.944,2.092,2.108-.936,2.108-2.092,2.108Zm15.043,12.919h-3.521v-5.997c0-1.644-.625-2.563-1.926-2.563-1.416,0-2.155.956-2.155,2.563v5.997h-3.393v-11.423h3.393v1.539s1.02-1.888,3.444-1.888,4.158,1.48,4.158,4.54v7.233h0Z'/></g><path fill='{INK}' d='m4.305,27.428c-2.374,0-4.305-1.932-4.305-4.306V4.306C0,1.932,1.931,0,4.305,0h18.817c2.374,0,4.305,1.932,4.305,4.306v18.816c0,2.374-1.931,4.306-4.305,4.306H4.305Zm.182-25.676c-1.508,0-2.736,1.227-2.736,2.735v18.453c0,1.509,1.228,2.735,2.736,2.735h18.453c1.508,0,2.736-1.227,2.736-2.735V4.487c0-1.509-1.228-2.735-2.736-2.735H4.487Z'/></g></g></svg>"

// --- Asset helpers --------------------------------------------------------------
// Recolour a single-ink SVG (logo, icons) to a theme colour and load it.
#let _ink-svg(src, ink, ..sizing) = image(
  bytes(src.replace("{INK}", ink.to-hex())),
  format: "svg",
  ..sizing,
)
// The retro corner sweep with its five stripes set to the theme's colours.
// This is the exact graphic embedded in the Word template (word/media/image4.svg),
// drawn at Word's scale (57.2cm) so that only a corner of it shows on the page.
#let _sweep(cols) = {
  assert(cols.len() >= 5, message: "theme.stripes needs 5 colours")
  let s = _svg-sweep
  for (i, c) in cols.enumerate() { s = s.replace("{S" + str(i + 1) + "}", c.to-hex()) }
  image(bytes(s), format: "svg", width: 57.2cm)
}

// --- Bar chart ------------------------------------------------------------------
// Horizontal bar chart for figures: rows = ((label, value, "display", colour), ..).
// Theme-aware – the track and value text follow the current text colour. Wrap in
// #figure(..., caption: [...]) for a numbered caption.
#let hanso-barchart(rows, max: none, label-width: 3cm, value-width: 1.7cm) = context {
  assert(rows.len() > 0, message: "hanso-barchart needs at least one row")
  let fg = text.fill
  // Guarded divisor: all-zero data renders minimum slivers instead of panicking.
  let top = calc.max(if max == none { calc.max(..rows.map(r => r.at(1))) } else { max }, 1e-9)
  set text(size: 9.5pt)
  stack(
    spacing: 0.85em,
    ..rows.map(r => grid(
      columns: (label-width, 1fr, value-width),
      align: (left + horizon, left + horizon, right + horizon),
      column-gutter: 0.8em,
      text(weight: 500, r.at(0)),
      box(width: 100%, height: 1.05em, radius: 2.5pt, fill: fg.transparentize(92%), place(
        left + horizon,
        box(width: calc.min(calc.max(r.at(1) / top, 0.015), 1) * 100%, height: 1.05em, radius: 2.5pt, fill: r.at(3)),
      )),
      text(fill: fg.transparentize(12%), r.at(2)),
    )),
  )
}

// --- Document template ------------------------------------------------------------
#let hanso-doc(
  theme: light-theme,
  title: "Document Title",
  author: "Author Name",
  date: datetime.today(),
  org: "Hanso Pte Ltd",
  address: ("1 Phillip Street", "#08-00, Royal One Phillip", "Singapore 048692"),
  phone: "+65 8890 8896",
  email: "info@hanso.group",
  web: "www.hanso.group",
  social: "@thehansogroup",
  bank: (
    "Bank: DBS Bank Ltd Singapore",
    "Address: 12 Marina Blvd, SG 018982",
    "Swift Code: DBSSSGSGXXX",
    "Account No.: 033-905982-0",
  ),
  // Footer style: "full" (default) is the 3-column address/contact/bank block for
  // official correspondence; "simple" is a single line for internal docs (change
  // logs, training material) – support email · website · confidentiality · social.
  footer-style: "full",
  support: "support@hanso.group", // support email shown in the simple footer
  confidentiality: "Internal use only", // classification note in the simple footer
  body,
) = {
  let fg = theme.fg

  // The corner sweep, placed with the Word master's transform: pulled up-left for
  // the page header, dropped low for the cover's bottom-right (yellow stays on top,
  // the maroon band touches the page edge).
  let sweep = _sweep(theme.stripes)
  let sweep-tl = place(top + left, dx: -16.17cm, dy: -4.83cm, sweep)
  let sweep-br = place(top + left, dx: -17.40cm, dy: 26.93cm, sweep)

  // Footer: "Page X of N" between two rules, shared by both styles.
  let contact(icon, label) = [#box(baseline: 0.18em, _ink-svg(icon, fg, height: 0.9em))#h(0.5em)#label]
  let rule-row = context {
    let last = counter(page).final().first()
    grid(
      columns: (1fr, auto, 1fr),
      align: horizon,
      column-gutter: 0.8em,
      line(length: 100%, stroke: 0.5pt + theme.rule),
      [Page #counter(page).display() of #last],
      line(length: 100%, stroke: 0.5pt + theme.rule),
    )
  }

  // Full footer: 3-column block – company + address | contact with icons | bank
  // details – whose rows share baselines. Wider than the text column (as in Word),
  // hence the negative pad. For official correspondence.
  let footer-full = pad(x: -1.6cm, {
    set text(weight: 300, size: 9pt, fill: fg)
    set par(leading: 0.5em, justify: false)
    rule-row
    v(5pt)
    let col-a = (org,) + address
    let col-b = (
      contact(_svg-ic-phone, phone),
      contact(_svg-ic-mail, email),
      contact(_svg-ic-web, web),
      contact(_svg-ic-linkedin, social),
    )
    let col-c = bank
    let rows = calc.max(col-a.len(), col-b.len(), col-c.len())
    grid(
      columns: (1fr, 1fr, 1fr),
      column-gutter: 1em,
      row-gutter: 0.85em,
      align: left + top,
      ..range(rows).map(i => (col-a.at(i, default: []), col-b.at(i, default: []), col-c.at(i, default: []))).flatten(),
    )
  })

  // Simple footer: the rule row, then a single line – support email · website ·
  // confidentiality · social handle. For internal docs (change logs, training
  // material) that don't need the full address and bank details.
  let footer-simple = pad(x: -1.6cm, {
    set text(weight: 300, size: 9pt, fill: fg)
    set par(leading: 0.5em, justify: false)
    rule-row
    v(6pt)
    grid(
      columns: (auto, auto, auto, auto),
      align: horizon,
      column-gutter: 1fr, // spread the four items evenly across the width
      contact(_svg-ic-mail, support),
      contact(_svg-ic-web, web),
      text(style: "italic")[#confidentiality],
      contact(_svg-ic-linkedin, social),
    )
  })

  let footer = if footer-style == "simple" { footer-simple } else { footer-full }

  set document(title: title, author: author)
  set text(font: "Figtree", weight: 300, size: 11pt, fill: fg, lang: "en") // body = Figtree Light
  set par(justify: true, leading: 1em, spacing: 1.7em)

  // Word master margins: 3.5cm sides, deep top for the sweep, deep bottom for the
  // full footer. The simple footer is a single line, so it reserves far less.
  let simple = footer-style == "simple"
  set page(
    paper: "a4",
    fill: theme.bg,
    margin: (left: 3.5cm, right: 3.5cm, top: 5cm, bottom: if simple { 4cm } else { 7cm }),
    background: sweep-tl,
    footer: footer,
    footer-descent: if simple { 1.6cm } else { 3cm },
  )

  // Headings: chapter I. II. III. (uppercase, new page) / section a. b. c. / 1. 2. 3.
  set heading(numbering: (..n) => {
    let pattern = ("I.", "a.", "1.").at(calc.min(n.pos().len(), 3) - 1)
    numbering(pattern, n.pos().last())
  })
  show heading: it => {
    let lvl = calc.min(it.level, 3)
    let blk = {
      // font + fill pinned so a body-level `set text` cannot restyle headings
      set text(font: "Figtree", fill: fg, weight: (900, 800, 800).at(lvl - 1), size: (28pt, 22pt, 14pt).at(lvl - 1))
      // Tight leading for wrapped headings – the body's 1em leading is far too airy
      // at display sizes. No justify so multi-line titles don't stretch.
      set par(leading: 0.32em, justify: false)
      block(
        above: if lvl == 1 { 0pt } else { 1.3em },
        below: (0.85em, 0.75em, 0.5em).at(lvl - 1),
        {
          if it.numbering != none { box[#counter(heading).display(it.numbering)#h(0.4em)] }
          if lvl == 1 { upper(it.body) } else { it.body }
        },
      )
    }
    if lvl == 1 {
      pagebreak(weak: true)
      v(0.5cm)
      blk
    } else { blk }
  }

  // Standfirst / tagline under a chapter title: centred accent italic, no rules.
  show quote.where(block: true): it => block(
    width: 100%,
    above: 0.6em,
    below: 1.6em,
    align(center, box(width: 80%, {
      set text(style: "italic", fill: theme.accent, size: 13pt, weight: 300)
      set par(leading: 0.75em, justify: false)
      it.body
      if it.attribution != none {
        v(0.35em)
        text(size: 11pt)[#sym.dash.en #it.attribution]
      }
    })),
  )

  // Lists: small brand dot; sources line etc. use plain body.
  set list(marker: text(size: 0.7em)[#sym.bullet], indent: 2pt, body-indent: 0.7em, spacing: 1.0em)
  set enum(indent: 2pt, body-indent: 0.7em)

  // Figures (images / photos / diagrams / tables): centred, with a numbered
  // caption – "Figure N – ..." with the label in the accent colour. @refs work.
  show figure: set block(above: 1.7em, below: 1.7em)
  set figure(gap: 0.9em)
  show figure.caption: it => context {
    // font + fill pinned so a body-level `set text` cannot restyle captions
    set text(font: "Figtree", fill: fg, size: 9.5pt)
    set par(leading: 0.55em, justify: false)
    if it.numbering != none {
      text(weight: 700, fill: theme.accent)[#it.supplement #it.counter.display(it.numbering)]
      [ #sym.dash.en ]
    }
    it.body
  }

  // Tables: an accent rule under the header row, faint hairlines between body
  // rows, no verticals; header cells bold.
  set table(
    stroke: (x, y) => (bottom: if y == 0 { 1pt + theme.accent } else { 0.5pt + fg.transparentize(72%) }),
    inset: (x: 0.7em, y: 0.62em),
    align: left + horizon,
  )
  show table.cell.where(y: 0): set text(weight: 700)

  // ------- Title page (full-bleed): footer/background off; both sweeps placed manually -------
  page(footer: none, background: none, margin: 0pt, {
    sweep-tl
    sweep-br
    place(center + horizon, dy: -0.5cm, {
      set align(center)
      _ink-svg(_svg-logo, fg, width: 10cm)
      v(-4pt)
      par(justify: false, leading: 0.3em, text(weight: 900, size: 36pt, hyphenate: false, upper(title)))
      v(-4pt)
      text(weight: 400, size: 18pt)[#author #h(0.2em) #box(text[•]) #h(0.2em) #date.display(
          "[day] [month repr:long] [year]",
        )]
    })
  })

  // ---------------- Table of contents ----------------
  show outline.entry: set text(weight: 400)
  show outline.entry: set block(above: 1.3em)
  show outline.entry.where(level: 1): set block(above: 1.75em)
  set outline.entry(fill: none) // no dot leaders
  outline(
    title: block(above: 0.7cm, below: 0.9cm, text(weight: 900, size: 28pt, upper[Table of Contents])),
    depth: 2,
    indent: 1.2em,
  )

  // ---------------- Body ----------------
  body
}
