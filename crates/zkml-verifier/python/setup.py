from setuptools import setup, find_packages

setup(
    name="zkml-verifier",
    version="0.1.0",
    description="Python wrapper for the ZKML proof verifier",
    packages=find_packages(),
    python_requires=">=3.9",
    classifiers=[
        "Development Status :: 3 - Alpha",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: Apache Software License",
        "Programming Language :: Python :: 3",
    ],
)
