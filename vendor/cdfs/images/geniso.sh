#!/usr/bin/env bash

set -e

# OSX:
# tar cvf ../joliet.tar -s '/.//' ./

# https://stackoverflow.com/questions/687014/removing-created-temp-files-in-unexpected-bash-exit
# Technicaly we only need bash 4.4+ but this is easier
if [ "${BASH_VERSINFO[0]}" -lt 5 ]; then
    echo "Need bash 5.x"
    exit 2
fi

INFILE=${1}
OUTFILE="$(basename "${1}" .tar).iso"

if [ "${INFILE}" = "" ]; then 
    echo "Input file needed"
    exit 1
fi

BUILD_DIR="$(mktemp -d)"
trap "rm -rf ${BUILD_DIR@Q}" EXIT

tar xvf "${INFILE}" -C ${BUILD_DIR}

pushd ${BUILD_DIR}
MKISOFS_FLAGS=$(cat flags.txt)
mkisofs -exclude flags.txt -exclude exclude.txt ${MKISOFS_FLAGS} -o "${OUTFILE}" .
popd

mv -i "${BUILD_DIR}/${OUTFILE}" .
