"""Convert the official legacy PaddleOCR inference graph without importing Paddle.

The paddle2onnx wheel contains a native converter which can parse an inference
pdmodel directly. Importing the package's public module also imports the much
larger Paddle framework, which is unnecessary for this one-time conversion.
"""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--converter-root", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--params", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_arguments()
    native_modules = list(
        args.converter_root.glob("paddle2onnx/paddle2onnx_cpp2py_export*.pyd")
    )
    if len(native_modules) != 1:
        raise RuntimeError(
            "Expected exactly one Paddle2ONNX native module under "
            f"{args.converter_root}; found {len(native_modules)}"
        )

    module_path = native_modules[0].resolve()
    spec = importlib.util.spec_from_file_location(
        "paddle2onnx_cpp2py_export", module_path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Could not load converter module: {module_path}")

    converter = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(converter)
    onnx_bytes = converter.export(
        str(args.model.resolve()),
        str(args.params.resolve()),
        14,
        True,
        True,
        True,
        True,
        True,
        {},
        "onnxruntime",
        "",
        "",
        False,
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(onnx_bytes)
    print(f"Wrote {len(onnx_bytes)} bytes to {args.output.resolve()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
