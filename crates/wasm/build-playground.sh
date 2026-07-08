#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "Building WASM..."
wasm-pack build . --target web --release

echo "Assembling playground..."
rm -rf _site
mkdir -p _site/pkg

cp pkg/wasm_bg.wasm _site/pkg/
cp pkg/wasm.js _site/pkg/
cp playground/index.html _site/
cp playground/monarch.js _site/
cp playground/editor.js _site/
cp playground/files.js _site/
cp playground/prefabs.js _site/
cp playground/share.js _site/
cp playground/docs.js _site/

echo "Copying docs + examples..."
mkdir -p _site/docs _site/sdk/examples
cp ../../docs/wirescript/*.md _site/docs/
node playground/build-search-index.mjs _site/docs _site/docs/search-index.json
cp playground/sdk/examples/*.ws _site/sdk/examples/

echo "Building SDK zip..."
rm -rf _sdk
mkdir -p _sdk/wirescript-sdk/pkg _sdk/wirescript-sdk/playground/pkg _sdk/wirescript-sdk/examples

cp pkg/wasm_bg.wasm _sdk/wirescript-sdk/pkg/
cp pkg/wasm.js _sdk/wirescript-sdk/pkg/
cp playground/sdk/check.mjs _sdk/wirescript-sdk/
cp playground/sdk/compile.mjs _sdk/wirescript-sdk/
cp playground/sdk/format.mjs _sdk/wirescript-sdk/
cp playground/sdk/hover.mjs _sdk/wirescript-sdk/
cp playground/sdk/CLAUDE.md _sdk/wirescript-sdk/
cp playground/sdk/README.md _sdk/wirescript-sdk/
cp playground/docs.js _sdk/wirescript-sdk/
cp playground/sdk/examples/*.ws _sdk/wirescript-sdk/examples/

cp playground/index.html _sdk/wirescript-sdk/playground/
cp playground/monarch.js _sdk/wirescript-sdk/playground/
cp playground/editor.js _sdk/wirescript-sdk/playground/
cp playground/files.js _sdk/wirescript-sdk/playground/
cp playground/prefabs.js _sdk/wirescript-sdk/playground/
cp playground/share.js _sdk/wirescript-sdk/playground/
cp playground/docs.js _sdk/wirescript-sdk/playground/
cp pkg/wasm_bg.wasm _sdk/wirescript-sdk/playground/pkg/
cp pkg/wasm.js _sdk/wirescript-sdk/playground/pkg/

mkdir -p _sdk/wirescript-sdk/vscode/syntaxes _sdk/wirescript-sdk/vscode/pkg
cp playground/sdk/vscode/extension.js _sdk/wirescript-sdk/vscode/
cp playground/sdk/vscode/package.json _sdk/wirescript-sdk/vscode/
cp ../../editors/vscode/language-configuration.json _sdk/wirescript-sdk/vscode/
cp ../../editors/vscode/syntaxes/wirescript.tmLanguage.json _sdk/wirescript-sdk/vscode/syntaxes/
cp pkg/wasm_bg.wasm _sdk/wirescript-sdk/vscode/pkg/
cp pkg/wasm.js _sdk/wirescript-sdk/vscode/pkg/

cd _sdk
zip -r ../wirescript-sdk.zip wirescript-sdk/ -x '*.DS_Store'
cd ..
rm -rf _sdk
cp wirescript-sdk.zip _site/

echo ""
echo "Done! Files:"
echo "  _site/               - serve with any HTTP server"
echo "  wirescript-sdk.zip   - downloadable SDK with CLI tools"
echo ""
echo "To test locally: cd _site && python3 -m http.server 8080"
