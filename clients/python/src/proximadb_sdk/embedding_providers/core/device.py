"""
Compute-device auto-detection.

Makes :attr:`ProviderConfig.device` ``None`` actually mean "auto-detect" (as its
docstring promises) instead of deferring to whatever default the underlying
library picks. Preference order: CUDA -> Apple MPS -> CPU.
"""

import logging

logger = logging.getLogger(__name__)


def resolve_device(device: str | None) -> str | None:
    """Resolve a device string, auto-detecting when ``device`` is None.

    Args:
        device: Explicit device ("cpu", "cuda", "mps", ...) or None to
            auto-detect.

    Returns:
        The resolved device string, or None if torch is unavailable (in which
        case the caller should let the backend choose its own default).
    """
    if device is not None:
        return device

    try:
        import torch
    except ImportError:
        # No torch (e.g. an ONNX-only install): let the backend default.
        return None

    try:
        if torch.cuda.is_available():
            return "cuda"
        mps = getattr(torch.backends, "mps", None)
        if mps is not None and mps.is_available():
            return "mps"
    except Exception as exc:  # pragma: no cover - defensive
        logger.debug("Device auto-detect fell back to CPU: %s", exc)

    return "cpu"
