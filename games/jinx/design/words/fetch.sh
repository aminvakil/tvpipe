#!/bin/sh
# Download the raw datasets into raw/ (not committed; see README.md).
set -eu
cd "$(dirname "$0")"
mkdir -p raw
cd raw
get() { [ -s "$1" ] || curl -fsSL -o "$1" "$2"; }

# CC BY 4.0
get Lancaster_sensorimotor_norms_for_39707_words.csv https://osf.io/download/48wsc/
get GlasgowNorms.xlsx https://osf.io/download/ud367/
get CYPLEX_all_age_bands.csv https://osf.io/download/8phz2/
get CYPLEX_README_main.txt https://osf.io/download/wn3ek/
get english-wordnet-2025.zip https://github.com/globalwordnet/english-wordnet/releases/download/2025-edition/english-wordnet-2025.zip
# SCOWL licence (permissive, see scowl-*/Copyright)
get scowl-2020.12.07.tar.gz https://downloads.sourceforge.net/wordlist/scowl-2020.12.07.tar.gz

[ -d oewn2025 ] || unzip -oq english-wordnet-2025.zip
[ -d scowl-2020.12.07 ] || tar xzf scowl-2020.12.07.tar.gz
echo "raw data ready"
