#!/usr/bin/env python3
import argparse
import json
import random
from pathlib import Path


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--osworld-dir", type=Path, required=True)
    p.add_argument("--n", type=int, default=30)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--domain", type=str, default="all")
    args = p.parse_args()

    meta_path = args.osworld_dir / "evaluation_examples" / "test_all.json"
    if not meta_path.exists():
        raise FileNotFoundError(f"not found: {meta_path}")

    meta = json.loads(meta_path.read_text())
    rng = random.Random(args.seed)

    tasks = []
    for domain, task_ids in meta.items():
        if args.domain != "all" and domain != args.domain:
            continue
        for task_id in task_ids:
            cfg = args.osworld_dir / "evaluation_examples" / "examples" / domain / f"{task_id}.json"
            if not cfg.exists():
                continue
            data = json.loads(cfg.read_text())
            instr = data.get("instruction", "")
            if not instr:
                continue
            tasks.append({"id": f"{domain}/{task_id}", "instruction": instr, "config_path": str(cfg)})

    rng.shuffle(tasks)
    selected = tasks[:args.n]
    args.output.write_text(json.dumps(selected, indent=2))
    print(f"wrote {len(selected)} tasks to {args.output}")
    for t in selected[:5]:
        print(f"  {t['id']}: {t['instruction'][:60]}")
    if len(selected) > 5:
        print(f"  ... {len(selected)-5} more")


if __name__ == "__main__":
    main()
