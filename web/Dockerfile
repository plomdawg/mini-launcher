FROM python:3.12-slim

WORKDIR /app
COPY . /app

RUN useradd --system --uid 10001 plomcase && chown -R plomcase:plomcase /app
USER plomcase

ENV PLOMCASE_HOST=0.0.0.0 \
    PORT=8784 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

EXPOSE 8784
CMD ["python", "server.py"]
