#!/usr/bin/env python3
"""
Common demo results logger for ProximaDB demos
Provides consistent logging and result tracking across all demos
"""

import json
import datetime
from pathlib import Path
from typing import Dict, Any, List, Optional
import sys


class DemoLogger:
    """Centralized logger for demo results"""
    
    def __init__(self, demo_name: str, log_dir: str = "demo/results"):
        self.demo_name = demo_name
        self.log_dir = Path(log_dir)
        self.log_dir.mkdir(exist_ok=True)
        
        # Create timestamped log file
        timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
        self.log_file = self.log_dir / f"{demo_name}_{timestamp}.json"
        self.console_log_file = self.log_dir / f"{demo_name}_{timestamp}.txt"
        
        self.results = {
            "demo_name": demo_name,
            "timestamp": timestamp,
            "start_time": datetime.datetime.now().isoformat(),
            "sections": [],
            "metrics": {},
            "errors": []
        }
        
        # Also setup console logging
        self.console_file = open(self.console_log_file, 'w')
        
    def section(self, title: str):
        """Start a new demo section"""
        print(f"\n{'='*60}")
        print(f"📌 {title}")
        print(f"{'='*60}")
        
        self.results["sections"].append({
            "title": title,
            "timestamp": datetime.datetime.now().isoformat(),
            "logs": []
        })
        
    def log(self, message: str, level: str = "info"):
        """Log a message"""
        icon = {
            "info": "ℹ️",
            "success": "✅",
            "warning": "⚠️",
            "error": "❌",
            "metric": "📊"
        }.get(level, "•")
        
        formatted_msg = f"{icon} {message}"
        print(formatted_msg)
        self.console_file.write(formatted_msg + "\n")
        
        if self.results["sections"]:
            self.results["sections"][-1]["logs"].append({
                "level": level,
                "message": message,
                "timestamp": datetime.datetime.now().isoformat()
            })
    
    def metric(self, name: str, value: Any, unit: str = ""):
        """Record a performance metric"""
        formatted_value = f"{value}{unit}" if unit else str(value)
        self.log(f"{name}: {formatted_value}", level="metric")
        
        self.results["metrics"][name] = {
            "value": value,
            "unit": unit,
            "timestamp": datetime.datetime.now().isoformat()
        }
    
    def error(self, message: str, exception: Optional[Exception] = None):
        """Log an error"""
        error_msg = message
        if exception:
            error_msg += f" - {type(exception).__name__}: {str(exception)}"
        
        self.log(error_msg, level="error")
        self.results["errors"].append({
            "message": message,
            "exception": str(exception) if exception else None,
            "timestamp": datetime.datetime.now().isoformat()
        })
    
    def success(self, message: str):
        """Log a success message"""
        self.log(message, level="success")
    
    def warning(self, message: str):
        """Log a warning"""
        self.log(message, level="warning")
    
    def finalize(self):
        """Finalize logging and save results"""
        self.results["end_time"] = datetime.datetime.now().isoformat()
        
        # Calculate duration
        start = datetime.datetime.fromisoformat(self.results["start_time"])
        end = datetime.datetime.fromisoformat(self.results["end_time"])
        duration = (end - start).total_seconds()
        self.results["duration_seconds"] = duration
        
        # Save JSON results
        with open(self.log_file, 'w') as f:
            json.dump(self.results, f, indent=2)
        
        # Close console log
        self.console_file.close()
        
        # Print summary
        print(f"\n{'='*60}")
        print(f"📋 Demo Summary: {self.demo_name}")
        print(f"{'='*60}")
        print(f"Duration: {duration:.2f} seconds")
        print(f"Sections completed: {len(self.results['sections'])}")
        print(f"Metrics recorded: {len(self.results['metrics'])}")
        print(f"Errors: {len(self.results['errors'])}")
        print(f"\nResults saved to:")
        print(f"  JSON: {self.log_file}")
        print(f"  Text: {self.console_log_file}")
        
        return self.results
    
    def __enter__(self):
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type:
            self.error("Demo failed with exception", exc_val)
        self.finalize()
        return False


def compare_results(demo_name: str, log_dir: str = "demo/results") -> Dict[str, Any]:
    """Compare results across multiple runs of the same demo"""
    log_path = Path(log_dir)
    demo_files = list(log_path.glob(f"{demo_name}_*.json"))
    
    if not demo_files:
        return {"error": f"No results found for demo: {demo_name}"}
    
    results = []
    for file in sorted(demo_files)[-5:]:  # Last 5 runs
        with open(file, 'r') as f:
            results.append(json.load(f))
    
    # Extract key metrics for comparison
    comparison = {
        "demo_name": demo_name,
        "runs_analyzed": len(results),
        "metrics_comparison": {},
        "duration_trend": []
    }
    
    # Compare metrics across runs
    all_metrics = set()
    for run in results:
        all_metrics.update(run.get("metrics", {}).keys())
    
    for metric in all_metrics:
        values = []
        for run in results:
            if metric in run.get("metrics", {}):
                values.append(run["metrics"][metric]["value"])
        
        if values and all(isinstance(v, (int, float)) for v in values):
            comparison["metrics_comparison"][metric] = {
                "min": min(values),
                "max": max(values),
                "avg": sum(values) / len(values),
                "latest": values[-1]
            }
    
    # Duration trend
    for run in results:
        if "duration_seconds" in run:
            comparison["duration_trend"].append({
                "timestamp": run["timestamp"],
                "duration": run["duration_seconds"]
            })
    
    return comparison