"""ProximaDB CLI - Command Line Interface for ProximaDB operations.

This module provides a comprehensive CLI for interacting with ProximaDB,
supporting both REST and gRPC protocols.

Usage:
    proximadb --help
    proximadb collections list
    proximadb vectors insert --collection my_collection --file vectors.json
    proximadb search --collection my_collection --query "[1.0, 2.0, ...]" --top-k 10
"""

import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

import click
from rich.console import Console
from rich.table import Table
from rich.panel import Panel
from rich.syntax import Syntax
from rich import print as rprint

# Local imports
from proximadb_sdk.config import ProximaDBConfig
from proximadb_sdk.unified_client import UnifiedProximaDBClient
from proximadb_sdk.models import SearchFilter

console = Console()


def get_client(
    host: str,
    rest_port: int,
    grpc_port: int,
    protocol: str,
    timeout: float,
) -> UnifiedProximaDBClient:
    """Create a ProximaDB client with specified configuration."""
    config = ProximaDBConfig(
        host=host,
        rest_port=rest_port,
        grpc_port=grpc_port,
        timeout=timeout,
    )
    return UnifiedProximaDBClient(config, preferred_protocol=protocol)


@click.group()
@click.option("--host", default="localhost", help="ProximaDB server host")
@click.option("--rest-port", default=5678, help="REST API port")
@click.option("--grpc-port", default=5679, help="gRPC API port")
@click.option(
    "--protocol",
    type=click.Choice(["auto", "grpc", "rest"]),
    default="auto",
    help="Protocol to use (auto selects best available)",
)
@click.option("--timeout", default=30.0, help="Request timeout in seconds")
@click.option("--json-output", is_flag=True, help="Output results as JSON")
@click.version_option(version="0.1.4", prog_name="ProximaDB CLI")
@click.pass_context
def cli(
    ctx: click.Context,
    host: str,
    rest_port: int,
    grpc_port: int,
    protocol: str,
    timeout: float,
    json_output: bool,
) -> None:
    """ProximaDB CLI - Cloud-native vector database for AI applications.

    Use this CLI to manage collections, insert vectors, and perform
    similarity searches against your ProximaDB server.
    """
    ctx.ensure_object(dict)
    ctx.obj["host"] = host
    ctx.obj["rest_port"] = rest_port
    ctx.obj["grpc_port"] = grpc_port
    ctx.obj["protocol"] = protocol
    ctx.obj["timeout"] = timeout
    ctx.obj["json_output"] = json_output


# --- Collection Commands ---


@cli.group()
def collections() -> None:
    """Manage ProximaDB collections."""
    pass


@collections.command("list")
@click.pass_context
def list_collections(ctx: click.Context) -> None:
    """List all collections in the database."""
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        result = client.list_collections()

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            if not result:
                console.print("[yellow]No collections found.[/yellow]")
                return

            table = Table(title="Collections")
            table.add_column("Name", style="cyan")
            table.add_column("Dimension", style="green")
            table.add_column("Vector Count", style="magenta")
            table.add_column("Storage Engine", style="blue")

            for coll in result:
                table.add_row(
                    coll.get("name", "N/A"),
                    str(coll.get("dimension", "N/A")),
                    str(coll.get("vector_count", "N/A")),
                    coll.get("storage_engine", "default"),
                )

            console.print(table)
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


@collections.command("create")
@click.argument("name")
@click.option("--dimension", "-d", required=True, type=int, help="Vector dimension")
@click.option(
    "--engine",
    type=click.Choice(["sst", "viper", "nova", "swift", "raptor", "helix"]),
    default="sst",
    help="Storage engine to use",
)
@click.option("--description", help="Collection description")
@click.pass_context
def create_collection(
    ctx: click.Context,
    name: str,
    dimension: int,
    engine: str,
    description: Optional[str],
) -> None:
    """Create a new collection."""
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        result = client.create_collection(
            name=name,
            dimension=dimension,
            storage_engine=engine,
            description=description,
        )

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            console.print(
                Panel(
                    f"[green]Collection '{name}' created successfully![/green]\n"
                    f"Dimension: {dimension}\n"
                    f"Engine: {engine}",
                    title="Success",
                )
            )
    except Exception as e:
        console.print(f"[red]Error creating collection: {e}[/red]")
        sys.exit(1)


@collections.command("delete")
@click.argument("name")
@click.option("--force", "-f", is_flag=True, help="Skip confirmation")
@click.pass_context
def delete_collection(ctx: click.Context, name: str, force: bool) -> None:
    """Delete a collection."""
    if not force:
        if not click.confirm(f"Are you sure you want to delete collection '{name}'?"):
            console.print("[yellow]Aborted.[/yellow]")
            return

    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        client.delete_collection(name)

        if ctx.obj["json_output"]:
            click.echo(json.dumps({"status": "deleted", "collection": name}))
        else:
            console.print(f"[green]Collection '{name}' deleted successfully.[/green]")
    except Exception as e:
        console.print(f"[red]Error deleting collection: {e}[/red]")
        sys.exit(1)


@collections.command("info")
@click.argument("name")
@click.pass_context
def collection_info(ctx: click.Context, name: str) -> None:
    """Get detailed information about a collection."""
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        result = client.get_collection(name)

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            table = Table(title=f"Collection: {name}")
            table.add_column("Property", style="cyan")
            table.add_column("Value", style="green")

            for key, value in result.items():
                table.add_row(key, str(value))

            console.print(table)
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


# --- Vector Commands ---


@cli.group()
def vectors() -> None:
    """Manage vectors in collections."""
    pass


@vectors.command("insert")
@click.option("--collection", "-c", required=True, help="Collection name")
@click.option("--file", "-f", type=click.Path(exists=True), help="JSON file with vectors")
@click.option("--vector", "-v", help="Single vector as JSON array")
@click.option("--id", "vector_id", help="Vector ID (for single vector)")
@click.option("--metadata", "-m", help="Metadata as JSON object")
@click.pass_context
def insert_vectors(
    ctx: click.Context,
    collection: str,
    file: Optional[str],
    vector: Optional[str],
    vector_id: Optional[str],
    metadata: Optional[str],
) -> None:
    """Insert vectors into a collection.

    Either provide a JSON file with vectors or a single vector.

    File format:
    [
        {"id": "vec1", "vector": [1.0, 2.0, ...], "metadata": {...}},
        ...
    ]
    """
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        vectors_data: List[Dict[str, Any]] = []

        if file:
            with open(file) as f:
                vectors_data = json.load(f)
        elif vector:
            vec = json.loads(vector)
            meta = json.loads(metadata) if metadata else {}
            vectors_data = [{"id": vector_id or "auto", "vector": vec, "metadata": meta}]
        else:
            console.print("[red]Either --file or --vector is required.[/red]")
            sys.exit(1)

        result = client.insert_vectors(collection, vectors_data)

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            console.print(
                f"[green]Successfully inserted {len(vectors_data)} vector(s) "
                f"into '{collection}'[/green]"
            )
    except Exception as e:
        console.print(f"[red]Error inserting vectors: {e}[/red]")
        sys.exit(1)


@vectors.command("get")
@click.option("--collection", "-c", required=True, help="Collection name")
@click.argument("vector_id")
@click.pass_context
def get_vector(ctx: click.Context, collection: str, vector_id: str) -> None:
    """Get a vector by ID."""
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        result = client.get_vector(collection, vector_id)

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            console.print(Panel(Syntax(json.dumps(result, indent=2), "json"), title=f"Vector: {vector_id}"))
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


@vectors.command("delete")
@click.option("--collection", "-c", required=True, help="Collection name")
@click.argument("vector_ids", nargs=-1)
@click.option("--force", "-f", is_flag=True, help="Skip confirmation")
@click.pass_context
def delete_vectors(
    ctx: click.Context,
    collection: str,
    vector_ids: tuple,
    force: bool,
) -> None:
    """Delete vectors by ID."""
    if not vector_ids:
        console.print("[red]At least one vector ID is required.[/red]")
        sys.exit(1)

    if not force:
        if not click.confirm(f"Delete {len(vector_ids)} vector(s) from '{collection}'?"):
            console.print("[yellow]Aborted.[/yellow]")
            return

    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        result = client.delete_vectors(collection, list(vector_ids))

        if ctx.obj["json_output"]:
            click.echo(json.dumps(result, indent=2))
        else:
            console.print(f"[green]Deleted {len(vector_ids)} vector(s) from '{collection}'[/green]")
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


# --- Search Commands ---


@cli.command("search")
@click.option("--collection", "-c", required=True, help="Collection name")
@click.option("--query", "-q", required=True, help="Query vector as JSON array")
@click.option("--top-k", "-k", default=10, help="Number of results to return")
@click.option("--filter", "-f", "filter_expr", help="Filter expression (JSON)")
@click.option(
    "--metric",
    type=click.Choice(["cosine", "euclidean", "dot_product"]),
    default="cosine",
    help="Distance metric",
)
@click.pass_context
def search(
    ctx: click.Context,
    collection: str,
    query: str,
    top_k: int,
    filter_expr: Optional[str],
    metric: str,
) -> None:
    """Perform similarity search.

    Example:
        proximadb search -c my_collection -q "[1.0, 2.0, 3.0]" -k 5
    """
    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        query_vector = json.loads(query)
        search_filter = None
        if filter_expr:
            search_filter = SearchFilter(**json.loads(filter_expr))

        results = client.search(
            collection_name=collection,
            query_vector=query_vector,
            top_k=top_k,
            filter=search_filter,
            distance_metric=metric,
        )

        if ctx.obj["json_output"]:
            click.echo(json.dumps(results, indent=2))
        else:
            table = Table(title=f"Search Results (top {top_k})")
            table.add_column("Rank", style="dim")
            table.add_column("ID", style="cyan")
            table.add_column("Score", style="green")
            table.add_column("Metadata", style="yellow")

            for i, result in enumerate(results, 1):
                metadata_str = json.dumps(result.get("metadata", {}))[:50]
                if len(json.dumps(result.get("metadata", {}))) > 50:
                    metadata_str += "..."
                table.add_row(
                    str(i),
                    result.get("id", "N/A"),
                    f"{result.get('score', 0):.4f}",
                    metadata_str,
                )

            console.print(table)
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


# --- Server Commands ---


@cli.group()
def server() -> None:
    """Server management commands."""
    pass


@server.command("health")
@click.pass_context
def health_check(ctx: click.Context) -> None:
    """Check server health status."""
    import httpx

    try:
        url = f"http://{ctx.obj['host']}:{ctx.obj['rest_port']}/health"
        response = httpx.get(url, timeout=ctx.obj["timeout"])

        if response.status_code == 200:
            if ctx.obj["json_output"]:
                click.echo(json.dumps({"status": "healthy", "details": response.json()}))
            else:
                console.print("[green]Server is healthy![/green]")
                console.print(Panel(Syntax(json.dumps(response.json(), indent=2), "json"), title="Health Details"))
        else:
            console.print(f"[yellow]Server returned status {response.status_code}[/yellow]")
    except Exception as e:
        console.print(f"[red]Server unreachable: {e}[/red]")
        sys.exit(1)


@server.command("info")
@click.pass_context
def server_info(ctx: click.Context) -> None:
    """Get server information."""
    import httpx

    try:
        url = f"http://{ctx.obj['host']}:{ctx.obj['rest_port']}/api/v1/info"
        response = httpx.get(url, timeout=ctx.obj["timeout"])

        if ctx.obj["json_output"]:
            click.echo(response.text)
        else:
            data = response.json()
            table = Table(title="Server Information")
            table.add_column("Property", style="cyan")
            table.add_column("Value", style="green")

            for key, value in data.items():
                table.add_row(key, str(value))

            console.print(table)
    except Exception as e:
        console.print(f"[red]Error: {e}[/red]")
        sys.exit(1)


# --- Utility Commands ---


@cli.command("benchmark")
@click.option("--collection", "-c", required=True, help="Collection name")
@click.option("--dimension", "-d", default=768, help="Vector dimension")
@click.option("--count", "-n", default=1000, help="Number of vectors to insert")
@click.option("--queries", "-q", default=100, help="Number of search queries")
@click.pass_context
def benchmark(
    ctx: click.Context,
    collection: str,
    dimension: int,
    count: int,
    queries: int,
) -> None:
    """Run a simple benchmark against the server."""
    import time
    import numpy as np

    try:
        client = get_client(
            ctx.obj["host"],
            ctx.obj["rest_port"],
            ctx.obj["grpc_port"],
            ctx.obj["protocol"],
            ctx.obj["timeout"],
        )

        console.print(f"[cyan]Running benchmark on '{collection}'...[/cyan]")

        # Create collection if needed
        try:
            client.create_collection(name=collection, dimension=dimension)
            console.print(f"  Created collection '{collection}'")
        except Exception:
            console.print(f"  Using existing collection '{collection}'")

        # Insert vectors
        console.print(f"  Inserting {count} vectors...")
        vectors = []
        for i in range(count):
            vectors.append({
                "id": f"bench_{i}",
                "vector": np.random.randn(dimension).tolist(),
                "metadata": {"index": i},
            })

        start = time.time()
        client.insert_vectors(collection, vectors)
        insert_time = time.time() - start

        # Search
        console.print(f"  Running {queries} search queries...")
        query_times = []
        for _ in range(queries):
            query = np.random.randn(dimension).tolist()
            start = time.time()
            client.search(collection, query, top_k=10)
            query_times.append(time.time() - start)

        avg_query_time = sum(query_times) / len(query_times)

        # Results
        results = {
            "insert_total_time": f"{insert_time:.2f}s",
            "insert_vectors_per_second": f"{count / insert_time:.0f}",
            "search_avg_latency": f"{avg_query_time * 1000:.2f}ms",
            "search_qps": f"{1 / avg_query_time:.0f}",
        }

        if ctx.obj["json_output"]:
            click.echo(json.dumps(results, indent=2))
        else:
            console.print("\n[green]Benchmark Results:[/green]")
            table = Table()
            table.add_column("Metric", style="cyan")
            table.add_column("Value", style="green")

            for key, value in results.items():
                table.add_row(key.replace("_", " ").title(), value)

            console.print(table)

    except Exception as e:
        console.print(f"[red]Benchmark failed: {e}[/red]")
        sys.exit(1)


def main() -> None:
    """Main entry point for the CLI."""
    cli(obj={})


if __name__ == "__main__":
    main()
