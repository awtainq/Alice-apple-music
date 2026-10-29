PYTHON ?= python3

.PHONY: test run

test:
	$(PYTHON) -m unittest discover -s tests -p 'test_*.py' -v

run:
	$(PYTHON) service.py --private private --cache cache
