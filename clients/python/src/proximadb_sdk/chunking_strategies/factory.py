"""
Factory for creating chunking strategies

Provides a clean interface for instantiating chunking strategies.
"""

from .base import ChunkingConfig, ChunkingStrategy, ChunkingStrategyInterface
from .code import CodeChunkingConfig, CodeChunkingStrategy
from .fixed_size import FixedSizeStrategy
from .paragraph import ParagraphStrategy
from .recursive import RecursiveStrategy
from .semantic import SemanticStrategy
from .sentence import SentenceStrategy
from .sliding_window import SlidingWindowStrategy


class ChunkingStrategyFactory:
    """Factory for creating chunking strategy instances"""

    # Registry of available strategies
    _strategies: dict[ChunkingStrategy, type[ChunkingStrategyInterface]] = {
        ChunkingStrategy.SLIDING_WINDOW: SlidingWindowStrategy,
        ChunkingStrategy.SENTENCE: SentenceStrategy,
        ChunkingStrategy.PARAGRAPH: ParagraphStrategy,
        ChunkingStrategy.SEMANTIC: SemanticStrategy,
        ChunkingStrategy.RECURSIVE: RecursiveStrategy,
        ChunkingStrategy.FIXED_SIZE: FixedSizeStrategy,
        ChunkingStrategy.CODE: CodeChunkingStrategy,
    }

    @classmethod
    def create_strategy(
        cls, strategy: ChunkingStrategy, config: ChunkingConfig | None = None
    ) -> ChunkingStrategyInterface:
        """
        Create a chunking strategy instance

        Args:
            strategy: The strategy type to create
            config: Optional configuration (uses defaults if not provided)

        Returns:
            Instance of the requested chunking strategy

        Raises:
            ValueError: If strategy type is not supported
        """
        if strategy not in cls._strategies:
            raise ValueError(
                f"Unknown chunking strategy: {strategy}. "
                f"Available strategies: {list(cls._strategies.keys())}"
            )

        # Use provided config or create default with the requested strategy
        if config is None:
            # For CODE strategy, use CodeChunkingConfig
            if strategy == ChunkingStrategy.CODE:
                config = CodeChunkingConfig(strategy=strategy)
            else:
                config = ChunkingConfig(strategy=strategy)
        else:
            # Ensure config has the correct strategy
            config.strategy = strategy
            # For CODE strategy, convert to CodeChunkingConfig if needed
            if strategy == ChunkingStrategy.CODE and not isinstance(
                config, CodeChunkingConfig
            ):
                # Create CodeChunkingConfig with base config values
                config = CodeChunkingConfig(
                    strategy=strategy,
                    chunk_size=config.chunk_size,
                    chunk_overlap=config.chunk_overlap,
                    min_chunk_size=config.min_chunk_size,
                    max_chunk_size=config.max_chunk_size,
                )

        # Create and return strategy instance
        strategy_class = cls._strategies[strategy]
        return strategy_class(config)

    @classmethod
    def register_strategy(
        cls, strategy: ChunkingStrategy, strategy_class: type[ChunkingStrategyInterface]
    ) -> None:
        """
        Register a custom chunking strategy

        Args:
            strategy: The strategy enum value
            strategy_class: The strategy implementation class
        """
        cls._strategies[strategy] = strategy_class

    @classmethod
    def list_strategies(cls) -> list[str]:
        """List all available chunking strategies"""
        return [s.value for s in cls._strategies.keys()]


def get_chunking_strategy(
    strategy: str | ChunkingStrategy = ChunkingStrategy.SLIDING_WINDOW, **kwargs
) -> ChunkingStrategyInterface:
    """
    Convenience function to get a chunking strategy

    Args:
        strategy: Strategy name or enum value
        **kwargs: Configuration parameters for ChunkingConfig

    Returns:
        Configured chunking strategy instance

    Example:
        chunker = get_chunking_strategy(
            "semantic",
            chunk_size=1000,
            chunk_overlap=100,
            preserve_code_blocks=True
        )
    """
    # Convert string to enum if needed
    if isinstance(strategy, str):
        strategy = ChunkingStrategy(strategy)

    # Create config from kwargs
    config = ChunkingConfig(strategy=strategy, **kwargs)

    # Create and return strategy
    return ChunkingStrategyFactory.create_strategy(strategy, config)
